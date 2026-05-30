/*
 * epub-reader-glue.js — Omnibus epub.js bridge.
 *
 * Depends on two globals that MUST be loaded as siblings BEFORE this file:
 *   - `ePub`  (epubjs@0.3.93)  — vendored at ./epub.min.js
 *   - `JSZip` (jszip@3.10.1)   — vendored at ./jszip.min.js
 *
 * The Rust reader page sets two optional callbacks on `window`:
 *   - `__omnibusOnRelocate(cfi)` — invoked (debounced) on every epub.js
 *     "relocated" event so the position can be persisted.
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

  // Report a lifecycle state to the Rust side if it registered a handler.
  // Defensive: a throwing/absent callback must never break rendering.
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

  function init(elementId, fileUrl, opts) {
    opts = opts || {};

    if (typeof ePub !== "function") {
      emitStatus("error");
      throw new Error("OmnibusReader.init: global `ePub` is not available");
    }

    // Re-init guard: tear down any prior rendition/book first.
    teardown();

    try {
      // `openAs: "epub"` is required: epub.js otherwise sniffs the URL's file
      // extension to decide how to open, and our route (/api/ebooks/:uuid/file)
      // has no `.epub` extension — without this it never fetches+unzips the
      // binary, so book.ready hangs and nothing renders.
      book = ePub(fileUrl, { openAs: "epub" });
      rendition = book.renderTo(elementId, {
        width: "100%",
        height: "100%",
        flow: "paginated",
        spread: "auto",
        allowScriptedContent: false,
      });

      rendition.themes.register("light", {
        body: { background: "#ffffff", color: "#141414" },
      });
      rendition.themes.register("dark", {
        body: { background: "#0f1115", color: "#e6e6e6" },
      });
      rendition.themes.register("sepia", {
        body: { background: "#f4ecd8", color: "#5b4636" },
      });
      rendition.themes.select(opts.theme || "dark");

      if (opts.fontSize) {
        rendition.themes.fontSize(opts.fontSize + "px");
      }
    } catch (e) {
      emitStatus("error");
      return;
    }

    // A bad fetch (404/expired session) or a malformed EPUB rejects here;
    // surface it instead of spinning on a blank viewer forever.
    book.ready.catch(function () {
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
        var cfi =
          location && location.start ? location.start.cfi : undefined;
        if (cfi && typeof window.__omnibusOnRelocate === "function") {
          window.__omnibusOnRelocate(cfi);
        }
      }, 400);
    });
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
