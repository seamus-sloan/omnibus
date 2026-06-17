/*
 * epub-reader-glue.js — Omnibus epub.js bridge.
 *
 * Depends on two globals that MUST be loaded as siblings BEFORE this file:
 *   - `ePub`  (epubjs@0.3.93)  — vendored at ./epub.min.js
 *   - `JSZip` (jszip@3.10.1)   — vendored at ./jszip.min.js
 *
 * The Rust reader page sets two optional callbacks on `window`:
 *   - `__omnibusOnRelocate(json)` — invoked (debounced) on every epub.js
 *     "relocated" event with a JSON string containing CFI + page/chapter
 *     data so the Rust side can update the bottom bar and persist position.
 *   - `__omnibusOnStatus(state)` — invoked with "ready" once the first page
 *     paints, or "error" if the book fails to open/render. Lets the Rust side
 *     drive a loading/error UI instead of leaving a blank viewer.
 * Same-origin .epub fetch is performed by epub.js via XHR, so the session
 * cookie is sent automatically.
 *
 * Public surface: window.OmnibusReader
 *   init(elementId, fileUrl, opts)  opts = { cfi?, fontSize?, theme?,
 *                                           fontFamily?, lineHeight?,
 *                                           maxWidth?, justify? }
 *   next()
 *   prev()
 *   setFontSize(px)
 *   setTheme(name)
 *   setFont(family)
 *   setLineHeight(value)
 *   setMargins(maxWidth)
 *   setJustify(on)
 *   addAnnotation(cfiRange, color)
 *   removeAnnotation(cfiRange)
 *   clearAnnotations()
 *   destroy()
 *
 * Selection callback:
 *   - `__omnibusOnSelection(json)` — invoked when the user selects text,
 *     with { cfiRange, rect: { x, y, width } } where rect is in viewport
 *     coordinates.
 */
(function () {
  "use strict";

  var book = null;
  var rendition = null;
  var relocateTimer = null;
  var locationsReady = false;
  var tocFlat = [];

  function emitStatus(state) {
    if (typeof window.__omnibusOnStatus === "function") {
      try {
        window.__omnibusOnStatus(state);
      } catch (e) {
        /* ignore handler errors */
      }
    }
  }

  function teardown() {
    if (relocateTimer) {
      clearTimeout(relocateTimer);
      relocateTimer = null;
    }
    locationsReady = false;
    tocFlat = [];
    if (rendition) {
      try {
        rendition.destroy();
      } catch (e) {
        /* ignore teardown errors */
      }
      rendition = null;
    }
    if (book) {
      try {
        book.destroy();
      } catch (e) {
        /* ignore teardown errors */
      }
      book = null;
    }
  }

  function flattenToc(items, out) {
    if (!items) return;
    for (var i = 0; i < items.length; i++) {
      out.push(items[i]);
      if (items[i].subitems) flattenToc(items[i].subitems, out);
    }
  }

  function findChapter(href) {
    if (!tocFlat.length || !href) return null;
    var clean = href.split("#")[0];
    for (var i = tocFlat.length - 1; i >= 0; i--) {
      var tocHref = (tocFlat[i].href || "").split("#")[0];
      if (tocHref === clean) {
        return { index: i + 1, total: tocFlat.length, title: tocFlat[i].label.trim() };
      }
    }
    return null;
  }

  function buildRelocateData(location) {
    var cfi = location && location.start ? location.start.cfi : undefined;
    var pct = location && location.start ? Math.round((location.start.percentage || 0) * 100) : 0;
    var page = 0;
    var totalPages = 0;
    if (locationsReady && book && book.locations) {
      page = book.locations.locationFromCfi(cfi) || 0;
      totalPages = book.locations.total || 0;
    }
    var ch = location && location.start ? findChapter(location.start.href) : null;
    return {
      cfi: cfi,
      page: page + 1,
      totalPages: totalPages,
      pct: pct,
      chapter: ch ? ch.index : 0,
      totalChapters: ch ? ch.total : tocFlat.length,
      chapterTitle: ch ? ch.title : "",
    };
  }

  function init(elementId, fileUrl, opts) {
    opts = opts || {};

    if (typeof ePub !== "function") {
      emitStatus("error");
      throw new Error("OmnibusReader.init: global `ePub` is not available");
    }

    teardown();

    try {
      book = ePub(fileUrl, { openAs: "epub" });
      rendition = book.renderTo(elementId, {
        width: "100%",
        height: "100%",
        flow: "paginated",
        spread: "auto",
        allowScriptedContent: false,
      });

      rendition.themes.register("light", {
        body: { background: "#fcfbfa", color: "#2a2725" },
      });
      rendition.themes.register("dark", {
        body: { background: "#201e1b", color: "#f5f3f0" },
      });
      rendition.themes.register("sepia", {
        body: { background: "#ede4d0", color: "#3b3029" },
      });
      rendition.themes.select(opts.theme || "dark");

      if (opts.fontSize) {
        rendition.themes.fontSize(opts.fontSize + "px");
      }
      if (opts.fontFamily) setFont(opts.fontFamily);
      if (opts.lineHeight) setLineHeight(opts.lineHeight);
      if (opts.maxWidth) setMargins(opts.maxWidth);
      if (opts.justify !== undefined) setJustify(opts.justify);
    } catch (e) {
      emitStatus("error");
      return;
    }

    book.ready
      .then(function () {
        tocFlat = [];
        if (book.navigation && book.navigation.toc) {
          flattenToc(book.navigation.toc, tocFlat);
        }
        return book.locations.generate(1024);
      })
      .then(function () {
        locationsReady = true;
        // Re-emit current location now that locations are resolved so the
        // Rust side gets real page numbers on first load.
        if (rendition && rendition.location) {
          emitRelocate(rendition.location);
        }
      })
      .catch(function () {
        emitStatus("error");
      });

    rendition.display(opts.cfi || undefined).then(
      function () {
        emitStatus("ready");
      },
      function () {
        emitStatus("error");
      }
    );

    rendition.on("relocated", function (location) {
      if (relocateTimer) {
        clearTimeout(relocateTimer);
      }
      relocateTimer = setTimeout(function () {
        relocateTimer = null;
        emitRelocate(location);
      }, 400);
    });

    rendition.on("selected", function (cfiRange, contents) {
      if (typeof window.__omnibusOnSelection !== "function") return;
      if (!contents || !contents.window) return;
      var sel = contents.window.getSelection();
      if (!sel || sel.isCollapsed || !sel.toString().trim()) return;
      var range = sel.getRangeAt(0);
      var iframeRect = { x: 0, y: 0 };
      try {
        var frame = contents.content.ownerDocument.defaultView.frameElement;
        if (frame) {
          var fr = frame.getBoundingClientRect();
          iframeRect = { x: fr.left, y: fr.top };
        }
      } catch (e) { /* cross-origin safety */ }
      var r = range.getBoundingClientRect();
      var rect = {
        x: r.left + iframeRect.x,
        y: r.top + iframeRect.y,
        width: r.width,
      };
      window.__omnibusOnSelection(JSON.stringify({
        cfiRange: cfiRange,
        rect: rect,
      }));
    });
  }

  function emitRelocate(location) {
    var data = buildRelocateData(location);
    if (data.cfi && typeof window.__omnibusOnRelocate === "function") {
      window.__omnibusOnRelocate(JSON.stringify(data));
    }
  }

  function next() {
    if (!rendition) return;
    rendition.next();
  }

  function prev() {
    if (!rendition) return;
    rendition.prev();
  }

  function setFontSize(px) {
    if (!rendition) return;
    rendition.themes.fontSize(px + "px");
  }

  function setTheme(name) {
    if (!rendition) return;
    rendition.themes.select(name);
  }

  function setFont(family) {
    if (!rendition) return;
    rendition.themes.font(family);
  }

  function setLineHeight(value) {
    if (!rendition) return;
    rendition.themes.override("line-height", value);
  }

  function setMargins(maxWidth) {
    if (!rendition) return;
    rendition.themes.override("max-width", maxWidth);
    rendition.themes.override("margin", "0 auto");
  }

  function setJustify(on) {
    if (!rendition) return;
    rendition.themes.override("text-align", on ? "justify" : "start");
  }

  // Solid fills — transparency is applied once via the `fill-opacity`
  // attribute below. Baking alpha into the color too would multiply with
  // fill-opacity (0.3 x 0.3) and render the highlight nearly invisible.
  var HIGHLIGHT_COLORS = {
    amber:  "rgb(245, 158, 11)",
    green:  "rgb(34, 197, 94)",
    blue:   "rgb(59, 130, 246)",
    rose:   "rgb(244, 63, 94)",
    violet: "rgb(139, 92, 246)",
  };

  function addAnnotation(cfiRange, color) {
    if (!rendition) return;
    var fill = HIGHLIGHT_COLORS[color] || HIGHLIGHT_COLORS.amber;
    rendition.annotations.add(
      "highlight", cfiRange, {}, undefined,
      "hl-" + color,
      { fill: fill, "fill-opacity": "0.3", "mix-blend-mode": "multiply" }
    );
  }

  function removeAnnotation(cfiRange) {
    if (!rendition) return;
    rendition.annotations.remove(cfiRange, "highlight");
  }

  function clearAnnotations() {
    if (!rendition || !rendition.annotations) return;
    var store = rendition.annotations._annotations;
    if (!store) return;
    var keys = Object.keys(store);
    for (var i = 0; i < keys.length; i++) {
      var entry = store[keys[i]];
      if (entry && entry.type === "highlight") {
        rendition.annotations.remove(entry.cfiRange, "highlight");
      }
    }
  }

  function destroy() {
    if (!rendition) return;
    teardown();
  }

  window.OmnibusReader = {
    init: init,
    next: next,
    prev: prev,
    setFontSize: setFontSize,
    setTheme: setTheme,
    setFont: setFont,
    setLineHeight: setLineHeight,
    setMargins: setMargins,
    setJustify: setJustify,
    addAnnotation: addAnnotation,
    removeAnnotation: removeAnnotation,
    clearAnnotations: clearAnnotations,
    destroy: destroy,
  };
})();
