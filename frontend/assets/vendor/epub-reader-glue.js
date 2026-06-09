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
 *   init(elementId, fileUrl, opts)  opts = { cfi?, fontSize?, theme? }
 *   next()
 *   prev()
 *   setFontSize(px)
 *   setTheme(name)
 *   destroy()
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
    destroy: destroy,
  };
})();
