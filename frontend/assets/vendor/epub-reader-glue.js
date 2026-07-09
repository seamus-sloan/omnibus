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
 *                                           maxWidth?, justify?,
 *                                           allowScriptedContent? }
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
 *   requestToc()                    re-emits __omnibusOnToc
 *   display(target)                 navigate to a TOC href or CFI
 *   copyText(text)                  clipboard write
 *   shareText(text)                 navigator.share, clipboard fallback
 *   exportQuoteCard(json)           canvas PNG → <a download>
 *   shareQuoteCard(json)            canvas PNG → navigator.share(files),
 *                                   download fallback
 *   copyQuoteCardImage(json)        canvas PNG → clipboard, download fallback
 *   destroy()
 *
 * Selection callback:
 *   - `__omnibusOnSelection(json)` — invoked when the user selects text,
 *     with { cfiRange, text, rect: { x, y, width } } where rect is in
 *     viewport coordinates.
 * Table-of-contents callback:
 *   - `__omnibusOnToc(json)` — invoked once the book is ready (and on
 *     requestToc()), with a flat [{ label, href, level }] array.
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
        // Without `allow-scripts` on the section iframe, WebKit dispatches NO
        // events to listeners inside it — gestures AND text selection are dead
        // on iOS. The mobile shell opts in (its books are the user's own
        // library); the web build keeps the stricter sandbox since desktop
        // engines still fire parent-attached listeners.
        allowScriptedContent: !!opts.allowScriptedContent,
      });

      installGestureNav();

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
      if (opts.spread) rendition.spread(opts.spread);
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
        emitToc();
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
        text: sel.toString(),
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

  // Touch page-turn for mobile: a horizontal swipe turns the page, and a tap in
  // the outer 20% gutters pages forward (right) / back (left). Registered on
  // every rendered section's iframe document via epub.js's content hook, so it
  // covers the prose the reader actually shows. Touch-only, so desktop (mouse +
  // the visible gutter buttons) is untouched; mobile CSS hides those buttons.
  function installGestureNav() {
    if (!rendition || !rendition.hooks || !rendition.hooks.content) return;
    rendition.hooks.content.register(function (contents) {
      var doc = contents.document;
      var win = contents.window || window;
      var sx = 0, sy = 0, st = 0;
      doc.addEventListener("touchstart", function (e) {
        var t = e.changedTouches[0];
        sx = t.clientX; sy = t.clientY; st = Date.now();
      }, { passive: true });
      doc.addEventListener("touchend", function (e) {
        // Never hijack the gesture that just made a text selection — that's the
        // highlight/note flow, not a page turn.
        var sel = win.getSelection && win.getSelection();
        if (sel && String(sel).length > 0) return;
        var t = e.changedTouches[0];
        var dx = t.clientX - sx, dy = t.clientY - sy, dt = Date.now() - st;
        // A dominant horizontal swipe turns a page (swipe left = forward).
        if (Math.abs(dx) > 45 && Math.abs(dx) > Math.abs(dy) * 1.5) {
          if (dx < 0) next(); else prev();
          return;
        }
        // Otherwise a stationary tap in the outer 20% gutters turns the page.
        // The section iframe is the whole multi-column spine item translated
        // as you page, so `clientX`/`innerWidth` are content coordinates —
        // map the tap into the app viewport through the iframe's rect and
        // compare against the app window's width instead.
        if (Math.abs(dx) < 10 && Math.abs(dy) < 10 && dt < 500) {
          var x = t.clientX;
          try {
            var fe = win.frameElement;
            if (fe) x += fe.getBoundingClientRect().left;
          } catch (err) { /* cross-origin safety */ }
          var w = window.innerWidth || 360;
          if (x > w * 0.8) next();
          else if (x < w * 0.2) prev();
        }
      }, { passive: true });
    });
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

  // Single vs two-page spread. "none" forces one column; "auto" lets epub.js
  // pair pages when the viewport is wide enough.
  function setSpread(mode) {
    if (!rendition) return;
    try {
      rendition.spread(mode);
    } catch (e) {
      /* older epub.js builds may not expose spread() */
    }
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

  // Walk the nested TOC into a flat [{label, href, level}] list and hand it
  // to the Rust side. Level is the nesting depth (0 = top), used to indent
  // the contents drawer. Re-emittable on demand via requestToc().
  function collectToc(items, level, out) {
    if (!items) return;
    for (var i = 0; i < items.length; i++) {
      out.push({
        label: (items[i].label || "").trim(),
        href: items[i].href || "",
        level: level,
      });
      if (items[i].subitems) collectToc(items[i].subitems, level + 1, out);
    }
  }

  function emitToc() {
    if (typeof window.__omnibusOnToc !== "function") return;
    var out = [];
    if (book && book.navigation && book.navigation.toc) {
      collectToc(book.navigation.toc, 0, out);
    }
    try {
      window.__omnibusOnToc(JSON.stringify(out));
    } catch (e) {
      /* ignore handler errors */
    }
  }

  function requestToc() {
    emitToc();
  }

  // Navigate to a TOC href or a CFI (highlight / bookmark target).
  // Display `target`, then re-display it once the section's fonts/theme have
  // settled. The first pass measures the target's column in a freshly
  // rendered iframe whose metrics can still shift (webfont swap, theme
  // injection) — the reflow leaves the viewport pages past the target, and
  // the follow-up display corrects it against the final layout.
  function displaySettled(target) {
    if (!rendition) return;
    rendition
      .display(target)
      .then(function () {
        var doc = null;
        try {
          var contents = rendition.getContents();
          var c = contents && contents[0];
          doc = c && c.document;
        } catch (e) {
          /* best effort */
        }
        var ready =
          doc && doc.fonts && doc.fonts.ready ? doc.fonts.ready : Promise.resolve();
        return ready
          .then(function () {
            return new Promise(function (res) {
              setTimeout(res, 80);
            });
          })
          .then(function () {
            if (rendition) return rendition.display(target);
          });
      })
      .catch(function () {
        /* target may be gone after a teardown */
      });
  }

  function display(target) {
    if (!rendition || !target) return;
    var t = String(target);
    var hash = t.indexOf("#");
    // CFIs and bare hrefs pass straight through. Fragment hrefs resolve to
    // the anchor's first *rendered* element first — Gutenberg-style TOCs
    // point at zero-size <a id> markers, and epub.js rounds an empty box to
    // the next column, landing a page past the chapter heading.
    if (hash > 0 && t.indexOf("epubcfi(") !== 0 && book && book.spine) {
      var section = book.spine.get(t.slice(0, hash));
      var id = t.slice(hash + 1);
      if (section) {
        section
          .load(book.load.bind(book))
          .then(function (doc) {
            var el = doc.getElementById(id);
            var probe = el;
            while (probe && !probe.childNodes.length) {
              probe = probe.nextElementSibling;
            }
            var cfi = null;
            try {
              cfi = section.cfiFromElement(probe || el);
            } catch (e) {
              /* fall back to the raw href below */
            }
            displaySettled(cfi || t);
          })
          .catch(function () {
            displaySettled(t);
          });
        return;
      }
    }
    displaySettled(t);
  }

  function copyText(text) {
    if (!text) return;
    try {
      if (navigator.clipboard && navigator.clipboard.writeText) {
        navigator.clipboard.writeText(text);
      }
    } catch (e) {
      /* clipboard unavailable */
    }
  }

  function shareText(text) {
    if (!text) return;
    try {
      if (navigator.share) {
        navigator.share({ text: text }).catch(function () {});
      } else {
        copyText(text);
      }
    } catch (e) {
      copyText(text);
    }
  }

  // In-book search. Walks each spine section, loads it, runs epub.js's
  // section.find(query), and hands a flat [{ cfi, excerpt, chapter }] array
  // back via __omnibusOnSearchResults. Section loads are throttled by the
  // promise chain so a large book doesn't open every section at once.
  function search(query) {
    if (typeof window.__omnibusOnSearchResults !== "function") return;
    if (!book || !book.spine || !query || !query.trim()) {
      window.__omnibusOnSearchResults(JSON.stringify([]));
      return;
    }
    var q = query.trim();
    var results = [];
    var chain = Promise.resolve();
    book.spine.each(function (section) {
      chain = chain.then(function () {
        if (results.length >= 80) return null;
        return section
          .load(book.load.bind(book))
          .then(function () {
            var found = section.find(q) || [];
            var chap = findChapter(section.href);
            for (var i = 0; i < found.length && results.length < 80; i++) {
              results.push({
                cfi: found[i].cfi,
                excerpt: (found[i].excerpt || "").trim(),
                chapter: chap ? chap.title : "",
              });
            }
            section.unload();
          })
          .catch(function () {
            /* skip unreadable section */
          });
      });
    });
    chain.then(function () {
      window.__omnibusOnSearchResults(JSON.stringify(results));
    });
  }

  // Render a quote card to a canvas and trigger a PNG download. Drawn directly
  // (rather than rasterizing DOM) because the card is a fixed, bespoke layout —
  // a solid background, an italic serif quote, and an attribution footer — so
  // hand-drawing yields crisp output with no heavyweight DOM-capture dependency.
  // Draw the quote card onto an offscreen canvas — the shared renderer behind
  // the export / share / copy actions. Returns null on a bad payload.
  function renderQuoteCanvas(json) {
    var o;
    try {
      o = JSON.parse(json);
    } catch (e) {
      return null;
    }
    var ratios = { "1:1": [1080, 1080], "4:5": [1080, 1350], "9:16": [1080, 1920], "3:4": [1080, 1440] };
    var dim = ratios[o.ratio] || ratios["1:1"];
    var W = dim[0], H = dim[1];
    var canvas = document.createElement("canvas");
    canvas.width = W;
    canvas.height = H;
    var ctx = canvas.getContext("2d");
    if (!ctx) return null;
    var pad = Math.round(W * 0.1);
    ctx.fillStyle = o.bg || "#1a1a1a";
    ctx.fillRect(0, 0, W, H);
    ctx.fillStyle = o.ink || "#ffffff";
    ctx.textBaseline = "top";

    // Header kicker.
    ctx.font = "600 " + Math.round(W * 0.018) + "px ui-monospace, monospace";
    ctx.globalAlpha = 0.55;
    ctx.fillText("OMNIBUS · QUOTE", pad, pad);
    ctx.globalAlpha = 1;

    // Quote body — word-wrapped italic serif.
    var quote = "“" + (o.text || "") + "”";
    var fontPx = Math.round(W * 0.052);
    ctx.font = "italic " + fontPx + "px Georgia, 'Times New Roman', serif";
    var maxW = W - pad * 2;
    var words = quote.split(/\s+/);
    var lines = [];
    var line = "";
    for (var i = 0; i < words.length; i++) {
      var test = line ? line + " " + words[i] : words[i];
      if (ctx.measureText(test).width > maxW && line) {
        lines.push(line);
        line = words[i];
      } else {
        line = test;
      }
    }
    if (line) lines.push(line);
    var lineH = Math.round(fontPx * 1.3);
    var blockH = lines.length * lineH;
    var startY = Math.max(pad * 2.2, (H - blockH) / 2 - lineH);
    for (var j = 0; j < lines.length; j++) {
      ctx.fillText(lines[j], pad, startY + j * lineH);
    }

    // Attribution footer.
    var footY = H - pad - Math.round(W * 0.06);
    ctx.globalAlpha = 0.85;
    ctx.fillRect(pad, footY - Math.round(W * 0.02), W - pad * 2, 2);
    ctx.font = Math.round(W * 0.022) + "px ui-sans-serif, system-ui, sans-serif";
    ctx.fillText((o.author || "").toUpperCase(), pad, footY);
    ctx.font = "italic " + Math.round(W * 0.026) + "px Georgia, serif";
    ctx.fillText(o.subtitle || "", pad, footY + Math.round(W * 0.035));
    ctx.globalAlpha = 1;
    return { canvas: canvas, name: (o.filename || "omnibus-quote") + ".png", title: o.subtitle || "Quote" };
  }

  function downloadBlob(blob, name) {
    var url = URL.createObjectURL(blob);
    var a = document.createElement("a");
    a.href = url;
    a.download = name;
    document.body.appendChild(a);
    a.click();
    document.body.removeChild(a);
    setTimeout(function () {
      URL.revokeObjectURL(url);
    }, 1000);
  }

  function exportQuoteCard(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      downloadBlob(blob, r.name);
    }, "image/png");
  }

  // Native share sheet with the rendered PNG. Falls back to a plain download
  // where Web Share can't take files (desktop browsers, older WebViews) —
  // WKWebView's programmatic `<a download>` is spotty, which is exactly why
  // the phone design routes through the share sheet instead.
  function shareQuoteCard(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      try {
        var f = new File([blob], r.name, { type: "image/png" });
        if (navigator.canShare && navigator.canShare({ files: [f] })) {
          navigator.share({ files: [f], title: r.title }).catch(function () {
            /* user dismissed the sheet */
          });
          return;
        }
      } catch (e) {
        /* File/Web Share unsupported */
      }
      downloadBlob(blob, r.name);
    }, "image/png");
  }

  // Copy the rendered PNG to the clipboard; falls back to a download when
  // ClipboardItem is unavailable.
  function copyQuoteCardImage(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    r.canvas.toBlob(function (blob) {
      if (!blob) return;
      try {
        if (navigator.clipboard && window.ClipboardItem) {
          var item = new ClipboardItem({ "image/png": blob });
          navigator.clipboard.write([item]).catch(function () {
            downloadBlob(blob, r.name);
          });
          return;
        }
      } catch (e) {
        /* ClipboardItem unsupported */
      }
      downloadBlob(blob, r.name);
    }, "image/png");
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
    setSpread: setSpread,
    addAnnotation: addAnnotation,
    removeAnnotation: removeAnnotation,
    clearAnnotations: clearAnnotations,
    requestToc: requestToc,
    display: display,
    copyText: copyText,
    shareText: shareText,
    search: search,
    exportQuoteCard: exportQuoteCard,
    shareQuoteCard: shareQuoteCard,
    copyQuoteCardImage: copyQuoteCardImage,
    destroy: destroy,
  };
})();
