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
 *                                           allowScriptedContent?,
 *                                           locationsKey? }
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
 *   display(target)                 navigate to a TOC href or CFI (also the
 *                                   target of in-book content-link taps —
 *                                   see installContentLinkNav())
 *   clearSelection()                drop the live selection
 *   copyText(text)                  clipboard write
 *   shareText(text)                 navigator.share, clipboard fallback
 *   exportQuoteCard(json)           canvas PNG → <a download>
 *   shareQuoteCard(json)            canvas PNG → navigator.share(files),
 *                                   download fallback
 *   copyQuoteCardImage(json)        canvas PNG → clipboard, download fallback
 *   destroy()
 *
 * Selection API (host-drawn selection; see the engine below):
 *   beginSelectionAt(x, y)          long-press: select the token at a point
 *   extendSelectionTo(x, y)         drag from the anchor token
 *   beginEdgeDrag(edge)             pin the opposite edge for a handle drag
 *   endSelectionDrag()              settle and re-emit with `existing`
 *   clearSelection()
 *
 * Selection callbacks:
 *   - `__omnibusOnSelection(json)` — the live range, as
 *     { cfiRange, text, rects: [{x,y,width,height}], start, end, existing,
 *       dragging } in HOST-WINDOW coordinates. One rect per visual line;
 *     `start`/`end` are the caret boxes the host hangs its handles off.
 *   - `__omnibusOnSelectionCleared(_)` — the range collapsed (tap-away,
 *     page turn), so the host can drop its selection UI.
 * Table-of-contents callback:
 *   - `__omnibusOnToc(json)` — invoked once the book is ready (and on
 *     requestToc()), with a flat [{ label, href, level }] array.
 * Native-share shims (optional; mobile shell only):
 *   - `__omnibusOnShareText(text)` / `__omnibusOnShareImage(json)` — when
 *     defined, shareText/shareQuoteCard route here instead of the Web Share
 *     API (absent in WKWebView) and the host presents the OS share sheet.
 *     The image payload is { name, dataUrl } with a base64 PNG data URL.
 */
(function () {
  "use strict";

  var book = null;
  var rendition = null;
  var relocateTimer = null;
  var locationsReady = false;
  // False while an initial CFI restore is still settling — mutes emitRelocate
  // so the first-pass landing (a page or two off until fonts/theme reflow) is
  // never persisted as reading progress.
  var restoreSettled = true;
  var tocFlat = [];
  var currentTheme = "dark";
  // Reading typography, tracked because the highlight geometry depends on it
  // — the leading between lines is what a mark has to grow to cover.
  var currentFontSize = 19;
  var currentLineHeight = 1.6;
  // Whether the section iframe runs scripts. Only then can we await the
  // iframe's `fonts.ready`, which is what makes the settle short enough to
  // hide behind a fade rather than a long blank.
  var scriptedContentAllowed = false;
  // Re-paginates when the viewer CONTAINER resizes without a window resize —
  // epub.js only listens for window resize, so the immersive audio dock
  // appearing/disappearing mid-read (which shrinks/grows `.rd-stage`) would
  // otherwise leave the last line of prose clipped behind stale pagination.
  var stageResizeObserver = null;
  var stageResizeTimer = null;
  // Live host-drawn selection, or null. See the selection engine below.
  var sel = null;
  var selEmitRaf = 0;
  // Spine item the reader is in, as the part of a CFI before its "!". Used to
  // skip annotations that can't be on this page without resolving them.
  var currentSectionBase = null;

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
    if (stageResizeTimer) {
      clearTimeout(stageResizeTimer);
      stageResizeTimer = null;
    }
    if (stageResizeObserver) {
      try {
        stageResizeObserver.disconnect();
      } catch (e) {
        /* ignore teardown errors */
      }
      stageResizeObserver = null;
    }
    cancelTurnAnim();
    endSectionTurn();
    endSettleFade();
    locationsReady = false;
    sectionRanges = null;
    tocFlat = [];
    sel = null;
    currentSectionBase = null;
    dropFlatCache();
    if (selEmitRaf) {
      cancelAnimationFrame(selEmitRaf);
      selEmitRaf = 0;
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

  // First and last whole-book location index of each spine section, derived
  // once the locations pass lands. epub.js knows how the whole book paginates
  // but not where a section begins and ends inside that, which is what
  // "N pages left in this chapter" needs.
  var sectionRanges = null;

  function buildSectionRanges() {
    sectionRanges = null;
    if (!book || !book.locations) return;
    var list = book.locations._locations;
    if (!list || !list.length) {
      // `save()` is the public form of the same array — used when a future
      // epub.js drops the private field.
      try {
        list = JSON.parse(book.locations.save());
      } catch (e) {
        return;
      }
    }
    if (!list || !list.length) return;
    var ranges = {};
    for (var i = 0; i < list.length; i++) {
      // Everything before the "!" step of a location CFI is its spine base.
      var base = String(list[i]).split("!")[0];
      if (ranges[base]) ranges[base].last = i;
      else ranges[base] = { first: i, last: i };
    }
    sectionRanges = ranges;
  }

  // Pages between this one and the end of its section. 0 means "unknown or
  // already on the last page", which the host renders as no label at all.
  function pagesLeftInSection(cfi, index) {
    if (!sectionRanges || !cfi) return 0;
    var range = sectionRanges[String(cfi).split("!")[0]];
    if (!range) return 0;
    var left = range.last - index;
    return left > 0 ? left : 0;
  }

  function buildRelocateData(location) {
    var cfi = location && location.start ? location.start.cfi : undefined;
    var pct = location && location.start ? Math.round((location.start.percentage || 0) * 100) : 0;
    var page = 0;
    var totalPages = 0;
    var pagesLeft = 0;
    if (locationsReady && book && book.locations) {
      page = book.locations.locationFromCfi(cfi) || 0;
      totalPages = book.locations.total || 0;
      pagesLeft = pagesLeftInSection(cfi, page);
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
      chapterPagesLeft: pagesLeft,
    };
  }

  // Watch the mount element for container-driven size changes (the immersive
  // dock reflowing `.rd-stage`) and re-run epub.js pagination. Skips the
  // initial observe callback (rendition display is still settling) and
  // debounces so a CSS transition only re-paginates once, at its final size.
  function installStageResizeWatch(elementId) {
    if (typeof ResizeObserver !== "function") return;
    var el = document.getElementById(elementId);
    if (!el) return;
    var lastW = el.clientWidth;
    var lastH = el.clientHeight;
    stageResizeObserver = new ResizeObserver(function () {
      var w = el.clientWidth;
      var h = el.clientHeight;
      if (w === lastW && h === lastH) return;
      lastW = w;
      lastH = h;
      if (stageResizeTimer) clearTimeout(stageResizeTimer);
      stageResizeTimer = setTimeout(function () {
        stageResizeTimer = null;
        if (!rendition) return;
        try {
          rendition.resize();
        } catch (e) {
          /* ignore resize races during teardown */
        }
      }, 120);
    });
    stageResizeObserver.observe(el);
  }

  function init(elementId, fileUrl, opts) {
    opts = opts || {};
    scriptedContentAllowed = !!opts.allowScriptedContent;

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
      installContentEnhancements();
      installContentLinkNav();
      installStageResizeWatch(elementId);

      // These body backgrounds are mirrored by the per-theme `--rd-page`
      // token in atrium.css (the reader-surface ground) — change both
      // together or the chrome strips stop matching the page.
      rendition.themes.register("light", {
        body: { background: "#fcfbfa", color: foregroundColorForTheme("light") },
      });
      rendition.themes.register("dark", {
        body: { background: "#201e1b", color: foregroundColorForTheme("dark") },
      });
      // Matches Apple Books' dark reading theme on macOS: a pure-black
      // #000000 page with bright #ffffff text.
      rendition.themes.register("black", {
        body: { background: "#000000", color: foregroundColorForTheme("black") },
      });
      rendition.themes.register("sepia", {
        body: { background: "#ede4d0", color: foregroundColorForTheme("sepia") },
      });
      rendition.themes.select(opts.theme || "dark");
      currentTheme = opts.theme || "dark";
      applyHostGround(currentTheme);

      if (opts.fontSize) {
        rendition.themes.fontSize(opts.fontSize + "px");
        currentFontSize = opts.fontSize;
      }
      if (opts.fontFamily) setFont(opts.fontFamily);
      if (opts.lineHeight) currentLineHeight = Number(opts.lineHeight) || currentLineHeight;
      if (opts.lineHeight) rendition.themes.override("line-height", opts.lineHeight);
      if (opts.maxWidth) setMargins(opts.maxWidth);
      if (opts.justify !== undefined) setJustify(opts.justify);
      if (opts.spread) rendition.spread(opts.spread);
      // Last, so the mark geometry is derived from the typography the book
      // actually opened with rather than from the defaults.
      applyMarkStyles();
    } catch (e) {
      emitStatus("error");
      return;
    }

    // Page numbers come from epub.js "locations" — a whole-book pagination
    // pass that takes seconds on desktop and much longer in the mobile
    // WebView. Cache the result per book (keyed by the host-supplied
    // `locationsKey`, the book uuid) so only the very first open pays it.
    // Storage failures (quota, private mode) and corrupt entries just fall
    // back to regeneration. Caveat: replacing a book's file under the same
    // uuid can leave slightly stale page numbers until the entry is cleared.
    var locationsCacheKey = opts.locationsKey ? "omn.locs::" + opts.locationsKey : null;
    book.ready
      .then(function () {
        tocFlat = [];
        if (book.navigation && book.navigation.toc) {
          flattenToc(book.navigation.toc, tocFlat);
        }
        emitToc();
        var cached = null;
        if (locationsCacheKey) {
          try {
            cached = window.localStorage.getItem(locationsCacheKey);
          } catch (e) { /* storage unavailable */ }
        }
        if (cached) {
          try {
            book.locations.load(cached);
            return null;
          } catch (e) { /* corrupt cache — regenerate below */ }
        }
        return book.locations.generate(1024).then(function () {
          if (locationsCacheKey) {
            try {
              window.localStorage.setItem(locationsCacheKey, book.locations.save());
            } catch (e) { /* quota/unavailable — regenerate next open */ }
          }
        });
      })
      .then(function () {
        locationsReady = true;
        buildSectionRanges();
        // Re-emit now that each entry can carry its page and percent.
        emitToc();
        // Re-emit current location now that locations are resolved so the
        // Rust side gets real page numbers on first load.
        if (rendition && rendition.location) {
          emitRelocate(rendition.location);
        }
      })
      .catch(function () {
        emitStatus("error");
      });

    // Restoring a saved position must go through the same settle-then-
    // redisplay correction as TOC/link navigation: the first-pass display
    // measures a freshly rendered iframe whose metrics still shift (webfont
    // swap, injected book CSS), landing a page or two off — and the relocate
    // that follows would persist that drifted position over the real one.
    // Relocates stay muted (restoreSettled) until the corrective redisplay
    // lands, and `ready` is deferred to the same moment: the first-pass
    // landing, the fonts/CSS reflow, and the correction all happen behind
    // the opaque loading overlay instead of flashing in view. Books without
    // saved progress paint (and report ready) immediately.
    var initialCfi = opts.cfi || null;
    restoreSettled = !initialCfi;
    rendition.display(initialCfi || undefined).then(
      function () {
        if (!initialCfi) {
          emitStatus("ready");
          return;
        }
        var r = rendition;
        var unmute = function () {
          // Identity check: a teardown+init for another book may have
          // swapped the rendition while this restore was settling.
          if (rendition !== r || restoreSettled) return;
          restoreSettled = true;
          emitStatus("ready");
          // A relocate already in flight (debounce pending) carries the
          // freshest measured landing and will emit now that the mute is
          // lifted — don't shadow it with a snapshot. Otherwise emit the
          // measured current location; `rendition.location` can echo the
          // display target rather than the rendered viewport, so it is
          // only the fallback.
          if (relocateTimer) return;
          var loc = null;
          try {
            loc = rendition.currentLocation();
          } catch (e) {
            /* not ready yet */
          }
          if (loc && loc.start) {
            emitRelocate(loc);
          } else if (rendition.location) {
            emitRelocate(rendition.location);
          }
        };
        redisplayWhenSettled(initialCfi)
          .then(function () {
            return nudgeToTarget(initialCfi);
          })
          .catch(function () {
            /* keep the first-pass landing */
          })
          .then(unmute);
        // Fail-open: if the settle chain ever hangs (an epub.js display that
        // never resolves), the mute must not permanently stop progress
        // persistence, nor the deferred ready leave the loading overlay up
        // forever — worst case reverts to the uncorrected landing.
        setTimeout(unmute, 4000);
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

    // A new section means new text nodes, so every block flattened off the
    // old ones is dead weight.
    rendition.on("rendered", dropFlatCache);

    rendition.on("relocated", function (location) {
      // A selection belongs to the page it was made on: once the page turns,
      // its geometry is stale and the passage is no longer in front of the
      // reader. Books drops it at the same moment.
      clearSelection();
      try {
        currentSectionBase = String(location.start.cfi).split("!")[0];
      } catch (e) {
        currentSectionBase = null;
      }
    });

    // Arrow keys page the book when the content iframe has focus. Key events
    // inside the iframe never bubble to the host document (so the Rust surface
    // keydown handler can't see them) — epub.js forwards them here instead.
    const prevPageKeys = new Set(["ArrowLeft", "ArrowUp", "H", "K"]);
    const nextPageKeys = new Set(["ArrowRight", "ArrowDown", "L", "J"]);
    rendition.on("keyup", function (e) {
      if (!e || !e.key) return;
      // Letter keys arrive lowercase without Shift; uppercase single-char keys
      // so h/j/k/l match the sets (arrow-key names are multi-char, unchanged).
      var k = e.key.length === 1 ? e.key.toUpperCase() : e.key;

      if (prevPageKeys.has(k)) {
        prev();
        e.preventDefault?.();
      } else if (nextPageKeys.has(k)) {
        next();
        e.preventDefault?.();
      }
    });
  }

  function emitRelocate(location) {
    if (!restoreSettled) return;
    var data = buildRelocateData(location);
    if (data.cfi && typeof window.__omnibusOnRelocate === "function") {
      window.__omnibusOnRelocate(JSON.stringify(data));
    }
  }

  function next() {
    if (!rendition) return;
    return rendition.next();
  }

  function prev() {
    if (!rendition) return;
    return rendition.prev();
  }

  var sectionTurnStyleInstalled = false;

  // Keep the outgoing section's pixels over epub.js while it clears the old
  // iframe, lays out the adjacent section, and positions its landing page.
  // Without this atomic hand-off, a previous-section turn briefly exposes the
  // first page of that section before manager.prev() scrolls to its last page.
  function installSectionTurnStyle() {
    if (sectionTurnStyleInstalled || !document.head) return;
    sectionTurnStyleInstalled = true;
    var style = document.createElement("style");
    style.id = "__omnibus_section_turn";
    style.textContent =
      "::view-transition-old(root),::view-transition-new(root){animation:none;}" +
      "::view-transition-new(omnibus-page){animation:none;z-index:1;}" +
      "html.omn-section-next::view-transition-old(omnibus-page){" +
        "animation:omn-section-next 240ms cubic-bezier(.22,.72,.18,1) both;z-index:2;}" +
      "html.omn-section-prev::view-transition-old(omnibus-page){" +
        "animation:omn-section-prev 240ms cubic-bezier(.22,.72,.18,1) both;z-index:2;}" +
      "@keyframes omn-section-next{to{transform:translateX(-100%);}}" +
      "@keyframes omn-section-prev{to{transform:translateX(100%);}}" +
      "@media(prefers-reduced-motion:reduce){" +
        "html.omn-section-next::view-transition-old(omnibus-page)," +
        "html.omn-section-prev::view-transition-old(omnibus-page){animation:none;}}";
    document.head.appendChild(style);
  }

  function reportSectionLocation(result) {
    if (result && result.then && rendition && rendition.reportLocation) {
      result.then(rendition.reportLocation.bind(rendition));
    } else if (rendition && rendition.reportLocation) {
      rendition.reportLocation();
    }
  }

  var sectionTurnInFlight = false;
  var sectionTurnToken = 0;

  // Mark a section turn in flight so new gestures can't capture a scroll
  // base mid-layout (or fight the View Transition overlay). Returns the
  // matching release; a fail-open timeout clears a turn whose promises
  // never settle so gestures can't end up permanently disabled.
  function beginSectionTurn() {
    var token = ++sectionTurnToken;
    sectionTurnInFlight = true;
    var release = function () {
      if (sectionTurnToken === token) sectionTurnInFlight = false;
    };
    setTimeout(release, 1500);
    return release;
  }

  // Reset every bit of section-turn state (teardown, or a book swap while
  // a turn was mid-flight): the gesture gate, the direction classes, and
  // the container's view-transition-name, so a later mount starts clean
  // and no other future View Transition accidentally captures the page.
  function endSectionTurn() {
    sectionTurnToken++;
    sectionTurnInFlight = false;
    document.documentElement.classList.remove("omn-section-next", "omn-section-prev");
    if (rendition && rendition.manager && rendition.manager.container) {
      rendition.manager.container.style.viewTransitionName = "";
    }
  }

  // Whether the spine has a section on the `dir` side of the one being
  // shown. Defaults to true on any surprise so the manager keeps the last
  // word — callers only use `false` to skip a turn entirely.
  function hasAdjacentSection(manager, dir) {
    try {
      var view = manager.views && manager.views.last && manager.views.last();
      var section = view && view.section;
      if (!section) return true;
      return !!(dir > 0 ? section.next() : section.prev());
    } catch (e) {
      return true;
    }
  }

  // Cross a spine boundary explicitly. A requestAnimationFrame hand-off is
  // unreliable here because WKWebView can stop scheduling frames immediately
  // after the final compositor transform is cleared. Returns false without
  // turning when there is no adjacent section (the book's first/last page):
  // the manager call would be a no-op there, stranding the resisted drag
  // offset on screen — and the View Transition would slide a snapshot of
  // the page over an identical copy of itself.
  function turnAcrossSection(dir) {
    if (!rendition || !rendition.manager) return false;
    var manager = rendition.manager;
    if (!hasAdjacentSection(manager, dir)) return false;
    var release = beginSectionTurn();
    var runTurn = function () {
      return dir > 0 ? manager.next() : manager.prev();
    };

    // A same-document View Transition captures the old iframe before
    // manager.prev()/next() synchronously clears it, waits for the returned
    // layout promise, then reveals only the fully positioned destination.
    // The named page snapshot continues in the swipe direction; everything
    // else renders as a static snapshot of the settled destination for the
    // ~240ms transition (the page counter catches up when reportLocation
    // fires after the update).
    if (typeof document.startViewTransition === "function" && manager.container) {
      installSectionTurnStyle();
      manager.container.style.viewTransitionName = "omnibus-page";
      var turnClass = dir > 0 ? "omn-section-next" : "omn-section-prev";
      document.documentElement.classList.add(turnClass);
      var transition = null;
      try {
        transition = document.startViewTransition(runTurn);
      } catch (e) {
        document.documentElement.classList.remove(turnClass);
      }
      if (transition) {
        var updated = transition.updateCallbackDone;
        if (updated && updated.then) {
          updated.then(function () {
            if (rendition && rendition.reportLocation) rendition.reportLocation();
          }, function () {
            /* manager failure is reported by epub.js */
          });
        }
        var finished = transition.finished;
        if (finished && finished.then) {
          finished.then(function () {
            document.documentElement.classList.remove(turnClass);
            release();
          }, function () {
            document.documentElement.classList.remove(turnClass);
            release();
          });
        } else {
          release();
        }
        return true;
      }
    }

    // Call the manager immediately instead of queueing through rendition.
    // Its built-in prev() waits for layout before positioning the last page of
    // the prior section; its next() similarly lands at the next section start.
    var result = runTurn();
    reportSectionLocation(result);
    if (result && result.then) {
      result.then(release, release);
    } else {
      release();
    }
    return true;
  }

  // Google Fonts stylesheet for the app's reading typefaces. The parent
  // document loads these via atrium.css, but webfonts don't cascade into the
  // section iframe — so `themes.font("'Instrument Serif'…")` renders as the
  // Times fallback unless the face is also declared inside the iframe.
  var READER_FONTS_HREF =
    "https://fonts.googleapis.com/css2?family=Instrument+Serif:ital@0;1&" +
    "family=EB+Garamond:ital,wght@0,400;0,500;1,400;1,500&display=swap";

  // Inject the book's own stylesheets as inline <style>. epub.js rewrites each
  // section `<link>` to a `blob:` URL, but the sandboxed iframe's opaque origin
  // can't load a parent-minted blob (and the app CSP blocks fetching it too),
  // so the publisher CSS silently drops and prose renders as UA defaults. We
  // instead read the CSS straight out of epub.js's in-memory archive (JSZip)
  // and inline it — an inline <style> the iframe honours. Fire-and-forget and
  // fully guarded so a parsing hiccup can never stall or break the render.
  function inlineBookStylesheets(doc) {
    try {
      if (!book || !book.archive || !book.packaging || doc.__omnibusBookCss) {
        return;
      }
      doc.__omnibusBookCss = true;
      var manifest = book.packaging.manifest || {};
      var paths = [];
      Object.keys(manifest).forEach(function (id) {
        var item = manifest[id];
        if (item && item.type === "text/css" && item.href) {
          try {
            paths.push(book.resolve(item.href));
          } catch (e) {
            /* unresolvable href — skip */
          }
        }
      });
      paths.forEach(function (path) {
        book.archive
          .getText(path)
          .then(function (css) {
            if (!css || !doc.head) return;
            var style = doc.createElement("style");
            style.setAttribute("data-omnibus-book-css", "");
            style.textContent = css;
            // Prepend so book CSS sits ahead of the reader baseline, which
            // only touches html/body and should win any tie (e.g. hyphens).
            doc.head.insertBefore(style, doc.head.firstChild);
          })
          .catch(function () {
            /* unreadable asset — leave prose on UA defaults */
          });
      });
    } catch (e) {
      /* never let styling break rendering */
    }
  }

  // Reader-owned hyperlink colour. Apple/Kindle paint links with their own
  // accent and ignore the publisher's — so links stay a consistent, legible
  // colour instead of whatever hue (or `:hover` red) a given book's CSS ships.
  // Theme-aware: a dark-ground blue would wash out on the light/sepia grounds.
  function linkColorForTheme(name) {
    switch (name) {
      case "light":
      case "sepia":
        return "#2f6fd0";
      default:
        return "#6fa8e6";
    }
  }

  // Reader-owned foreground. Publisher styles often put `color` directly on a
  // classed body (e.g. Calibre's `.calibre{color:#000}`), whose specificity
  // beats epub.js's plain `body` theme rule.
  function foregroundColorForTheme(name) {
    switch (name) {
      case "light":
        return "#2a2725";
      case "black":
        return "#ffffff";
      case "sepia":
        return "#3b3029";
      default:
        return "#f5f3f0";
    }
  }

  // Push the current theme's reader-owned colours into a section as CSS vars
  // the baseline stylesheet reads. Runs per section and on every theme swap.
  function applyThemeColors(doc) {
    if (doc && doc.documentElement) {
      doc.documentElement.style.setProperty(
        "--omn-fg",
        foregroundColorForTheme(currentTheme)
      );
      doc.documentElement.style.setProperty(
        "--omn-link",
        linkColorForTheme(currentTheme)
      );
    }
  }

  // Per-section content enhancement, registered on epub.js's content hook so it
  // runs for every rendered spine item. Three Apple/Kindle-parity fixes the
  // sandboxed iframe would otherwise drop: the book's own stylesheet, then
  // hyphenation for justified prose (off by CSS default), and the app typeface
  // loaded *inside* the iframe.
  function installContentEnhancements() {
    if (!rendition || !rendition.hooks || !rendition.hooks.content) return;
    rendition.hooks.content.register(function (contents) {
      var doc = contents.document;
      if (!doc || !doc.head) return;

      inlineBookStylesheets(doc);

      // Hyphenation needs a language for its dictionary; inherit the book's,
      // defaulting to English, without clobbering a per-document `lang`.
      var meta = book && book.packaging && book.packaging.metadata;
      var lang = (meta && meta.language) || "en";
      if (doc.documentElement && !doc.documentElement.getAttribute("lang")) {
        doc.documentElement.setAttribute("lang", lang);
      }

      if (!doc.getElementById("__omnibus_fonts")) {
        var link = doc.createElement("link");
        link.id = "__omnibus_fonts";
        link.rel = "stylesheet";
        link.href = READER_FONTS_HREF;
        doc.head.appendChild(link);
      }

      applyThemeColors(doc);

      // Reader baseline / override layer. The split mirrors Apple Books and
      // Kindle: the reading system owns colour (and font, size, spacing,
      // margins, justification — set elsewhere), while the publisher keeps
      // structure — weight, style, headings, alignment, indents, small-caps.
      //
      // Appended last, and `!important` on colour so a publisher hue can't
      // override the theme. Include `body` itself: descendants inheriting from
      // a publisher-coloured, classed body otherwise stay black in Black/Dark
      // themes. Only *real* links — `a` with an `href` — get the reader's
      // accent. Scoping to `[href]` also spares body text that Gutenberg wraps
      // in a self-closing *named* anchor (`<a id="chapN"/>`, no href), which
      // the HTML parser leaves open across the chapter.
      if (!doc.getElementById("__omnibus_baseline")) {
        var style = doc.createElement("style");
        style.id = "__omnibus_baseline";
        style.textContent =
          "html,body{-webkit-hyphens:auto;-ms-hyphens:auto;hyphens:auto;}" +
          "body,body *{color:var(--omn-fg,#f5f3f0)!important;}" +
          "a:not([href]){cursor:auto;}" +
          "a[href]{color:var(--omn-link,#4a86d8)!important;text-decoration:none;}" +
          // WebKit's own touch selection is what made selecting a sentence
          // feel like a fight: its long-press recogniser and the drag-to-turn
          // handler claim the same touch, and its handles and loupe are laid
          // out against the iframe's full multi-column width, so they land in
          // the wrong column. Turned off here, it never engages — the glue's
          // selection engine owns the range and the host draws it.
          "html,body,body *{-webkit-user-select:none!important;" +
          "user-select:none!important;-webkit-touch-callout:none!important;}";
        doc.head.appendChild(style);
      }
    });
  }

  // ── Host-drawn text selection ──────────────────────────────────────
  // The glue owns the *range*; the host owns every pixel of it. WebKit's
  // selection is disabled in the section (see the baseline stylesheet), so
  // this engine reads geometry out of the DOM and the host draws the tint,
  // the handles, and the menu as real UIKit layers — which is what makes
  // selecting a sentence feel like the rest of the phone rather than like a
  // web page inside it.
  //
  // Everything crossing the bridge is in HOST-WINDOW coordinates: the host
  // has no notion of the section iframe, which in paginated flow is as wide
  // as the whole chapter and slides under the viewport as pages turn.

  // A selection boundary lands between tokens, and a token is a run of
  // non-space characters — punctuation included.
  //
  // Letters-and-digits was the obvious granule and the wrong one: a boundary
  // could then never land on a full stop, a comma or a quotation mark, so the
  // end of a sentence was literally unreachable and a lone dash could not be
  // selected at all.
  function isSpace(ch) {
    return ch === "" || /\s/.test(ch);
  }

  // Where a token may not run past. Anything else — a <span>, an <em>, a
  // footnote link — is inline markup that a token can legitimately cross.
  var BLOCK_TAGS = {
    P: 1, DIV: 1, LI: 1, BLOCKQUOTE: 1, SECTION: 1, ARTICLE: 1, ASIDE: 1,
    TD: 1, TH: 1, DD: 1, DT: 1, FIGCAPTION: 1, PRE: 1, BODY: 1,
    H1: 1, H2: 1, H3: 1, H4: 1, H5: 1, H6: 1,
  };

  function blockRootOf(node) {
    var el = node && node.nodeType === 1 ? node : node && node.parentNode;
    while (el && el.nodeType === 1) {
      if (BLOCK_TAGS[el.nodeName]) return el;
      el = el.parentNode;
    }
    return null;
  }

  // A block's text as one string, plus the map back to (text node, offset).
  // Token boundaries are then plain string arithmetic, and a word broken
  // across inline markup — `<em>hel</em>lo`, or a mid-word footnote anchor —
  // is still one token rather than two.
  function flatten(root, doc) {
    var walker = doc.createTreeWalker(root, NodeFilter.SHOW_TEXT, null, false);
    var segs = [];
    var text = "";
    var node;
    while ((node = walker.nextNode())) {
      if (!node.data || !node.data.length) continue;
      segs.push({ node: node, at: text.length, len: node.data.length });
      text += node.data;
    }
    return { text: text, segs: segs };
  }

  // Flattened blocks, keyed by the element they were built from.
  //
  // `extendSelectionTo` runs on every `touchmove`, and `blockRootOf` stops at
  // the nearest block tag — which in a book whose chapter is one undivided
  // <div> (plenty are) is the whole chapter. Rebuilt per event, that is the
  // chapter's text concatenated sixty times a second.
  //
  // Safe to hold across a drag because the index is purely textual: node
  // identity and offsets, no geometry. Re-pagination doesn't touch it; only a
  // new section does, which is what `rendered` below invalidates on.
  var flatCache = typeof Map === "function" ? new Map() : null;

  function flattenCached(root, doc) {
    if (!flatCache) return flatten(root, doc);
    var hit = flatCache.get(root);
    if (hit) return hit;
    var flat = flatten(root, doc);
    flatCache.set(root, flat);
    return flat;
  }

  function dropFlatCache() {
    if (flatCache) flatCache.clear();
  }

  function flatIndexOf(flat, node, offset) {
    for (var i = 0; i < flat.segs.length; i++) {
      if (flat.segs[i].node === node) return flat.segs[i].at + offset;
    }
    return -1;
  }

  function flatPointAt(flat, index) {
    for (var i = 0; i < flat.segs.length; i++) {
      var seg = flat.segs[i];
      if (index <= seg.at + seg.len) {
        return { node: seg.node, offset: Math.max(0, index - seg.at) };
      }
    }
    var last = flat.segs[flat.segs.length - 1];
    return last ? { node: last.node, offset: last.len } : null;
  }

  // The whole token under a caret, as a Range.
  //
  // Snapping to tokens is what makes a drag feel deliberate: the selection
  // never stops mid-word, so it always reads as a phrase, and the reader can
  // aim at a word rather than at a character. Apple Books and Kindle both
  // select at this granularity for exactly that reason — and both carry the
  // punctuation, which is what lets a drag reach the end of a sentence.
  function tokenRangeAt(doc, node, offset) {
    var root = blockRootOf(node);
    if (!root) return null;
    var flat = flattenCached(root, doc);
    var index = flatIndexOf(flat, node, offset);
    if (index < 0) return null;

    var text = flat.text;
    var start = index;
    var end = index;
    if (isSpace(text.charAt(end))) {
      // The caret landed in the gap between tokens. Take the one behind it
      // when there is one — that is the one the finger was nearest — else
      // scan forward to the next.
      if (start > 0 && !isSpace(text.charAt(start - 1))) {
        end = start;
      } else {
        while (end < text.length && isSpace(text.charAt(end))) end++;
        start = end;
      }
    }
    while (start > 0 && !isSpace(text.charAt(start - 1))) start--;
    while (end < text.length && !isSpace(text.charAt(end))) end++;
    if (start === end) return null;

    var from = flatPointAt(flat, start);
    var to = flatPointAt(flat, end);
    if (!from || !to) return null;
    try {
      var range = doc.createRange();
      range.setStart(from.node, from.offset);
      range.setEnd(to.node, to.offset);
      return range;
    } catch (e) {
      return null;
    }
  }

  // The offset from host-window coordinates into a section's own viewport.
  function frameOffset(win) {
    try {
      var frame = win && win.frameElement;
      if (frame) {
        var r = frame.getBoundingClientRect();
        return { x: r.left, y: r.top };
      }
    } catch (e) {
      /* cross-origin safety */
    }
    return { x: 0, y: 0 };
  }

  function renderedContents() {
    var list = (rendition && rendition.getContents && rendition.getContents()) || [];
    if (!Array.isArray(list)) list = list && list.document ? [list] : [];
    return list;
  }

  function contentsForDocument(doc) {
    var list = renderedContents();
    for (var i = 0; i < list.length; i++) {
      if (list[i].document === doc) return list[i];
    }
    return list[0] || null;
  }

  // The text position under a point given in host-window coordinates.
  function caretAtHostPoint(x, y) {
    var list = renderedContents();
    for (var i = 0; i < list.length; i++) {
      var doc = list[i].document;
      var win = list[i].window;
      if (!doc || !win || !doc.caretRangeFromPoint) continue;
      var off = frameOffset(win);
      var caret = doc.caretRangeFromPoint(x - off.x, y - off.y);
      // Only a caret inside real text is something to snap a word to; a hit
      // on an image or on block padding resolves to an element node.
      if (caret && caret.startContainer && caret.startContainer.nodeType === 3) {
        return {
          doc: doc,
          win: win,
          node: caret.startContainer,
          offset: caret.startOffset,
        };
      }
    }
    return null;
  }

  // The range spanning two ranges, whichever order they were made in.
  function spanning(doc, a, b) {
    var aFirst = a.compareBoundaryPoints(Range.START_TO_START, b) <= 0;
    var first = aFirst ? a : b;
    var last = aFirst ? b : a;
    var range = doc.createRange();
    range.setStart(first.startContainer, first.startOffset);
    range.setEnd(last.endContainer, last.endOffset);
    return range;
  }

  // One rounded bar per visual line, in host-window coordinates.
  //
  // `getClientRects` fragments a line at every inline element boundary, so a
  // sentence crossing an <em> comes back as three abutting boxes — drawn as
  // given, they show seams and doubled corners where they meet.
  function lineRects(range, win) {
    var off = frameOffset(win);
    var raw = (range.getClientRects && range.getClientRects()) || [];
    var rows = [];
    for (var i = 0; i < raw.length; i++) {
      var r = raw[i];
      if (!r || r.width <= 0 || r.height <= 0) continue;
      var row = null;
      for (var k = 0; k < rows.length; k++) {
        // Same line iff the boxes share most of their height. Superscripts
        // and inline images sit on the line without matching its box.
        var overlap = Math.min(rows[k].bottom, r.bottom) - Math.max(rows[k].top, r.top);
        var shorter = Math.min(rows[k].bottom - rows[k].top, r.height);
        if (overlap > shorter * 0.5) {
          row = rows[k];
          break;
        }
      }
      if (row) {
        row.left = Math.min(row.left, r.left);
        row.right = Math.max(row.right, r.right);
        row.top = Math.min(row.top, r.top);
        row.bottom = Math.max(row.bottom, r.bottom);
      } else {
        rows.push({ left: r.left, right: r.right, top: r.top, bottom: r.bottom });
      }
    }

    // Reading order, so the host's handles hang off the true ends.
    rows.sort(function (a, b) {
      return a.top - b.top || a.left - b.left;
    });

    // `getClientRects` measures the *font* box, not the line box, so on
    // generously leaded prose the bars come back with a stripe of page
    // between them. Grow each row by the gap its neighbours leave, which
    // makes a multi-line selection one continuous block the way the system's
    // own is — without needing to know the line height.
    var gap = Infinity;
    for (var g = 1; g < rows.length; g++) {
      var between = rows[g].top - rows[g - 1].bottom;
      if (between > 0 && between < gap) gap = between;
    }
    if (gap !== Infinity && gap > 0) {
      var grow = gap / 2;
      for (var e = 0; e < rows.length; e++) {
        rows[e].top -= grow;
        rows[e].bottom += grow;
      }
    }

    // Only what is on the page in front of the reader: a selection near a
    // column edge also has rects in the next column, which is off-screen but
    // still inside the (chapter-wide) iframe.
    var container = pageContainer();
    var box = container ? container.getBoundingClientRect() : null;
    var out = [];
    for (var j = 0; j < rows.length; j++) {
      var x = rows[j].left + off.x;
      var width = rows[j].right - rows[j].left;
      if (box && (x + width / 2 < box.left - 1 || x + width / 2 > box.right + 1)) continue;
      out.push({
        x: x,
        y: rows[j].top + off.y,
        width: width,
        height: rows[j].bottom - rows[j].top,
      });
    }
    return out;
  }

  function hasSelection() {
    return !!(sel && sel.range && !sel.range.collapsed);
  }

  // `dragging` mutes the two expensive parts — the CFI and the
  // overlapping-highlight scan — while the finger is still moving; the host
  // only needs geometry and text until the drag settles.
  function emitSelection(dragging) {
    if (!sel || typeof window.__omnibusOnSelection !== "function") return;
    var rects = lineRects(sel.range, sel.win);
    if (!rects.length) return;
    var first = rects[0];
    var last = rects[rects.length - 1];
    var cfi = sel.cfiRange;
    var existing = sel.existing || null;
    if (!dragging) {
      try {
        cfi = sel.contents ? sel.contents.cfiFromRange(sel.range) : null;
      } catch (e) {
        cfi = null;
      }
      existing = overlappingAnnotation(sel.range);
      sel.cfiRange = cfi;
      sel.existing = existing;
    }
    try {
      window.__omnibusOnSelection(JSON.stringify({
        cfiRange: cfi,
        // Collapsed: a range's text carries the source file's own line
        // breaks and indentation, which are invisible on the page but come
        // out as ragged breaks anywhere the passage is re-set — a note, a
        // quote card, a paste.
        text: sel.range.toString().replace(/\s+/g, " ").trim(),
        rects: rects,
        start: { x: first.x, y: first.y, height: first.height },
        end: { x: last.x + last.width, y: last.y, height: last.height },
        existing: existing,
        dragging: !!dragging,
      }));
    } catch (e) {
      /* ignore handler errors */
    }
  }

  // At most one emit per frame while a finger is moving: `touchmove` outruns
  // the display, and each emit costs a layout read plus a bridge hop.
  function scheduleSelectionEmit() {
    if (selEmitRaf) return;
    selEmitRaf = requestAnimationFrame(function () {
      selEmitRaf = 0;
      emitSelection(true);
    });
  }

  function beginSelectionAt(x, y) {
    var caret = caretAtHostPoint(x, y);
    if (!caret) return false;
    var token = tokenRangeAt(caret.doc, caret.node, caret.offset);
    if (!token) return false;
    sel = {
      doc: caret.doc,
      win: caret.win,
      contents: contentsForDocument(caret.doc),
      anchor: token.cloneRange(),
      range: token,
      cfiRange: null,
      existing: null,
    };
    // Reported as a drag: the finger is still down, and the host holds its
    // menu back until the range settles rather than opening one under it.
    emitSelection(true);
    return hasSelection();
  }

  function extendSelectionTo(x, y) {
    if (!sel) return;
    var caret = caretAtHostPoint(x, y);
    // A point outside the section (the margins, the next column) leaves the
    // range where it was rather than collapsing it out from under the finger.
    if (!caret || caret.doc !== sel.doc) return;
    var token = tokenRangeAt(caret.doc, caret.node, caret.offset);
    if (!token) return;
    sel.range = spanning(sel.doc, sel.anchor, token);
    scheduleSelectionEmit();
  }

  // Pin the edge the host is *not* dragging, so the drag reads as moving one
  // end of the range. Pulling one end past the other then flips them, the way
  // a text field does, because the pinned edge is a fixed point to span from.
  function beginEdgeDrag(edge) {
    if (!sel) return;
    var anchor = sel.doc.createRange();
    if (edge === "start") {
      anchor.setStart(sel.range.endContainer, sel.range.endOffset);
    } else {
      anchor.setStart(sel.range.startContainer, sel.range.startOffset);
    }
    anchor.collapse(true);
    sel.anchor = anchor;
  }

  function endSelectionDrag() {
    if (selEmitRaf) {
      cancelAnimationFrame(selEmitRaf);
      selEmitRaf = 0;
    }
    emitSelection(false);
  }

  function clearSelection() {
    if (selEmitRaf) {
      cancelAnimationFrame(selEmitRaf);
      selEmitRaf = 0;
    }
    if (!sel) return;
    sel = null;
    if (typeof window.__omnibusOnSelectionCleared === "function") {
      try {
        window.__omnibusOnSelectionCleared("");
      } catch (e) {
        /* ignore handler errors */
      }
    }
  }

  // ── Books-style page-turn (touch) ──────────────────────────────────
  // The default paginated manager pages by moving `container.scrollLeft`
  // in steps of `layout.delta`, with the whole section rendered as
  // adjacent columns — so the neighbouring page's pixels already exist.
  // Motion never touches scrollLeft directly though: every scrollLeft
  // write fires the manager's scroll listener and its layout-reading
  // location machinery, which visibly stutters a slow drag. Instead the
  // drag and the snap animate a translateX on the container's children
  // (compositor-only, invisible to epub.js) and the landing commits ONE
  // scrollLeft assignment + transform clear in the same frame — one
  // scroll event, one relocation, so the page counter and progress
  // persistence ride the existing path exactly once per turn.
  // Section boundaries (first/last page of a chapter) have no adjacent
  // pixels, so a View Transition keeps the outgoing raster over epub.js while
  // the adjacent section lays out, then completes the slide over the ready
  // destination page.

  var turnAnim = null; // in-flight snap animation

  function pageContainer() {
    return (rendition && rendition.manager && rendition.manager.container) || null;
  }

  function pageDelta() {
    var m = rendition && rendition.manager;
    if (m && m.layout && m.layout.delta) return m.layout.delta;
    var c = pageContainer();
    return c ? c.clientWidth : 0;
  }

  function maxScroll(c) {
    return Math.max(0, c.scrollWidth - c.clientWidth);
  }

  // The drag/snap offset, as a translateX on every view child of the
  // scroll container (visually identical to scrolling the clipped box).
  function setViewOffset(c, px) {
    var scale = window.devicePixelRatio || 1;
    var aligned = Math.round(px * scale) / scale;
    for (var i = 0; i < c.children.length; i++) {
      c.children[i].style.transform = aligned ? "translate3d(" + aligned + "px,0,0)" : "";
    }
  }

  // Promote the views to their own compositor layer for the duration of a
  // gesture. Without this WebKit rasterizes the section iframe lazily
  // (visible strip only), so a slow drag exposes not-yet-painted content
  // popping in at the leading edge frame by frame; with a promoted layer
  // the section is rasterized once and the drag just shifts a texture.
  function armViews(c, on) {
    for (var i = 0; i < c.children.length; i++) {
      c.children[i].style.willChange = on ? "transform" : "";
    }
  }

  // Land the visual offset: one scrollLeft assignment + transform clear
  // in the same frame, so epub.js sees a single settled scroll.
  function commitOffset(c, base, px) {
    c.scrollLeft = Math.max(0, Math.min(maxScroll(c), base - px));
    setViewOffset(c, 0);
    armViews(c, false);
  }

  function cancelTurnAnim() {
    if (!turnAnim) return;
    var a = turnAnim;
    turnAnim = null;
    if (a.raf) cancelAnimationFrame(a.raf);
    setViewOffset(a.container, 0);
    armViews(a.container, false);
  }

  // Land an in-flight snap instantly (its target is already the settled
  // page edge, so this is a fast-forward, never a visual glitch).
  function finishTurnAnim() {
    if (!turnAnim) return;
    var a = turnAnim;
    turnAnim = null;
    if (a.raf) cancelAnimationFrame(a.raf);
    commitOffset(a.container, a.base, a.targetPx);
  }

  function animateOffsetTo(c, base, fromPx, toPx, ms) {
    finishTurnAnim();
    var reduced = window.matchMedia &&
      window.matchMedia("(prefers-reduced-motion: reduce)").matches;
    if (fromPx === toPx || reduced) {
      commitOffset(c, base, toPx);
      return;
    }
    armViews(c, true);
    var a = { container: c, base: base, targetPx: toPx, raf: 0 };
    var start = null;
    turnAnim = a;
    function easeOutCubic(t) { return 1 - Math.pow(1 - t, 3); }
    function step(ts) {
      if (turnAnim !== a) return;
      if (start === null) start = ts;
      var t = Math.min(1, (ts - start) / ms);
      if (t < 1) {
        setViewOffset(c, fromPx + (toPx - fromPx) * easeOutCubic(t));
        a.raf = requestAnimationFrame(step);
      } else {
        turnAnim = null;
        commitOffset(c, base, toPx);
      }
    }
    a.raf = requestAnimationFrame(step);
  }

  // Animate one page forward/back when the neighbouring page is rendered
  // in this section; use the atomic section hand-off at a boundary.
  function turnAnimated(dir) {
    var c = pageContainer();
    var d = pageDelta();
    if (!c || !d) {
      if (dir > 0) next(); else prev();
      return;
    }
    finishTurnAnim();
    var base = c.scrollLeft;
    var target = base + dir * d;
    if (target < -0.5 || target > maxScroll(c) + 0.5) {
      // A false return means the book's edge; the tap pulled nothing, so
      // there is no offset to settle — just don't turn.
      turnAcrossSection(dir);
      return;
    }
    animateOffsetTo(c, base, 0, -dir * d, 260);
  }

  // Touch page-turn for mobile: a horizontal drag slides the page under the
  // finger and snaps on release (past 30% of a page, or a flick), and a tap
  // in the outer 20% gutters plays the same animated turn. Attached to each
  // section iframe's document and the host reading surface, which also covers
  // the margins around the iframe. Touch-only, so desktop mouse navigation is
  // untouched; mobile CSS hides the visible gutter buttons.
  function installGestureNav() {
    if (!rendition) return;
    var attachAll = function () {
      var list = (rendition.getContents && rendition.getContents()) || [];
      if (!Array.isArray(list)) list = list && list.document ? [list] : [];
      for (var i = 0; i < list.length; i++) {
        installGestureHandlers(list[i].document, false);
      }
      return list.length;
    };
    // Attach inside each rendered section. Re-run idempotently after every
    // relocation because chapter changes swap the iframe document.
    rendition.on("rendered", attachAll);
    rendition.on("relocated", attachAll);
    // The host listener covers stage margins and the passive bottom bar. A
    // touch belongs to one document tree, so this cannot double-handle events
    // received by the section iframe.
    installGestureHandlers(document, true);
  }

  function installGestureHandlers(doc, isHost) {
    if (!doc || doc.__omnibusGestures) return;
    doc.__omnibusGestures = true;
    var sx = 0, sy = 0, st = 0;
    var dragAxis = null, dragBase = 0, dragVx = 0;
    var dragPx = 0, dragIntentPx = 0, dragPending = null, dragRaf = 0;
    var velocitySamples = [];
    // Long-press-to-select: armed on touchdown, disarmed by movement.
    var pressTimer = null, pressAt = null;
    // This touch is drawing a selection, so it is not a page gesture.
    var selecting = false;
    // This touch lands while a selection is up, so its job is to dismiss it.
    var dismissing = false;

    // A touch point in host-window coordinates. `clientX/Y` inside a section
    // is relative to an iframe that is as wide as the chapter and slides
    // under the viewport, so it has to be rebased before it crosses over.
    function hostPoint(t) {
      if (isHost) return { x: t.clientX, y: t.clientY };
      var off = { x: 0, y: 0 };
      try {
        var frame = (doc.defaultView || {}).frameElement;
        if (frame) {
          var r = frame.getBoundingClientRect();
          off = { x: r.left, y: r.top };
        }
      } catch (err) {
        /* cross-origin safety */
      }
      return { x: t.clientX + off.x, y: t.clientY + off.y };
    }

    function cancelPress() {
      if (pressTimer) {
        clearTimeout(pressTimer);
        pressTimer = null;
      }
    }

    function nowMs() {
      return window.performance && performance.now ? performance.now() : Date.now();
    }
    // `clientX` is relative to the moving iframe on iOS, so using it creates
    // a feedback loop: each transform changes the next gesture coordinate.
    function stableX(t) {
      if (typeof t.screenX === "number" && isFinite(t.screenX)) return t.screenX;
      var x = t.clientX;
      if (!isHost) {
        try {
          var fe = (doc.defaultView || {}).frameElement;
          if (fe) x += fe.getBoundingClientRect().left;
        } catch (err) { /* cross-origin safety */ }
      }
      return x;
    }
    function sampleVelocity(x, now) {
      velocitySamples.push({ x: x, t: now });
      while (velocitySamples.length > 2 && velocitySamples[1].t < now - 100) {
        velocitySamples.shift();
      }
      var first = velocitySamples[0];
      dragVx = now > first.t ? (x - first.x) / (now - first.t) : 0;
    }
    function stopDragRaf(flush) {
      if (dragRaf) {
        cancelAnimationFrame(dragRaf);
        dragRaf = 0;
      }
      if (flush && dragPending !== null) {
        var c = pageContainer();
        if (c) setViewOffset(c, dragPending);
      }
      dragPending = null;
    }
    // Apply at most one drag offset per frame — touchmove can outpace the
    // display, and per-event writes are wasted work.
    function applyDrag() {
      dragRaf = 0;
      if (dragAxis !== "x" || dragPending === null) return;
      var c = pageContainer();
      if (c) setViewOffset(c, dragPending);
    }
    function springBack() {
      stopDragRaf(true);
      if (dragAxis === "x") {
        var c = pageContainer();
        if (c) animateOffsetTo(c, dragBase, dragPx, 0, 180);
      }
      dragAxis = null;
    }

    // RTL books page with negative scroll offsets in some engines; keep
    // the classic instant swipe there rather than mis-dragging.
    var rtl = !!(book && book.packaging && book.packaging.metadata &&
      String(book.packaging.metadata.direction || "").toLowerCase() === "rtl");
    var skipTap = false;

    doc.addEventListener("touchstart", function (e) {
      cancelPress();
      selecting = false;
      dismissing = false;
      // A section turn is still laying out (or its View Transition is
      // holding the screen): a gesture started now would capture a stale
      // scroll base and fight the hand-off. Ignore the touch entirely.
      if (sectionTurnInFlight) {
        dragAxis = "none";
        skipTap = true;
        return;
      }
      // A live selection owns the screen. Its handles and menu are host
      // views that take their own touches, so anything reaching the page is
      // a tap-away — never a page turn under a menu about a passage on it.
      if (hasSelection()) {
        dragAxis = "none";
        skipTap = false;
        dismissing = true;
        if (e.touches && e.touches.length === 1) {
          var dt = e.touches[0];
          sx = stableX(dt);
          sy = dt.clientY;
          st = nowMs();
        }
        return;
      }
      // Host install: gestures over the reading surface (stage incl.
      // its margins, and the passive bottom bar) are page gestures;
      // buttons, links, sheets, and drawers keep their own touch
      // semantics untouched.
      if (isHost) {
        var tg = e.target;
        var onSurface = tg && tg.closest && tg.closest(".rd-stage, .rd-bottom");
        var onControl = tg && tg.closest && tg.closest("button, a, [role=\"button\"], input, textarea, .rd-drawer, .rd-aa-panel, .rd-note-composer, .rd-search-drawer, .m-sheet, .m-sheet-scrim");
        if (!onSurface || onControl) { dragAxis = "none"; skipTap = true; return; }
      }
      skipTap = false;
      if (!e.touches || e.touches.length !== 1) { dragAxis = "none"; return; }
      var t = e.touches[0];
      sx = stableX(t);
      sy = t.clientY;
      st = nowMs();
      dragAxis = null;
      dragPx = 0;
      dragIntentPx = 0;
      dragVx = 0;
      velocitySamples = [];
      // A new touch catches a settling page at its destination.
      finishTurnAnim();
      var c = pageContainer();
      dragBase = c ? c.scrollLeft : 0;

      // Arm the long press. Near UIKit's own 0.5s: short enough that holding
      // a word feels answered, long enough that a swipe which starts with a
      // moment's settle isn't mistaken for one. Movement past the slop in
      // `touchmove` disarms it, the way `allowableMovement` does.
      pressAt = hostPoint(t);
      pressTimer = setTimeout(function () {
        pressTimer = null;
        if (dragAxis !== null || !beginSelectionAt(pressAt.x, pressAt.y)) return;
        selecting = true;
        // The finger is now drawing a selection, so this touch can no longer
        // become a page drag or a tap.
        dragAxis = "none";
        skipTap = true;
      }, 420);
    }, { passive: true });

    doc.addEventListener("touchmove", function (e) {
      if (selecting) {
        // Keep the page still under a selection being drawn, and extend to
        // the word the finger is over.
        e.preventDefault();
        if (e.touches.length === 1) {
          var sp = hostPoint(e.touches[0]);
          extendSelectionTo(sp.x, sp.y);
        }
        return;
      }
      if (pressTimer && e.touches.length === 1) {
        // Movement past the press slop means this is a swipe, not a hold.
        var pt = e.touches[0];
        if (Math.abs(stableX(pt) - sx) > 10 || Math.abs(pt.clientY - sy) > 10) {
          cancelPress();
        }
      }
      if (rtl || dismissing || dragAxis === "none") return;
      if (e.touches.length !== 1) { springBack(); return; }
      var t = e.touches[0];
      var x = stableX(t);
      var dx = x - sx;
      if (dragAxis === null) {
        // The dead zone protects taps and long-press selection. The reader is
        // paginated, so late horizontal intent may engage despite vertical
        // finger drift.
        if (Math.abs(dx) < 8) return;
        dragAxis = "x";
        cancelPress();
        sx = x;
        sy = t.clientY;
        dx = 0;
        velocitySamples = [{ x: x, t: nowMs() }];
        var ac = pageContainer();
        if (ac) armViews(ac, true);
      }
      e.preventDefault();

      var now = nowMs();
      sampleVelocity(x, now);
      var c = pageContainer();
      if (!c) return;
      var d = pageDelta();
      var lo = Math.max(dragBase - maxScroll(c), d ? -d : -Infinity);
      var hi = Math.min(dragBase, d ? d : Infinity);
      dragIntentPx = d ? Math.max(-d, Math.min(d, dx)) : dx;

      // Within a section the page tracks the finger exactly. At a chapter
      // edge, a short resisted pull communicates the boundary while the full
      // gesture intent remains available to choose the next section.
      var resisted = dx;
      if (resisted < lo) resisted = lo + (resisted - lo) * 0.18;
      if (resisted > hi) resisted = hi + (resisted - hi) * 0.18;
      var edge = d ? Math.min(48, d * 0.12) : 48;
      dragPx = Math.max(lo - edge, Math.min(hi + edge, resisted));
      dragPending = dragPx;
      if (!dragRaf) dragRaf = requestAnimationFrame(applyDrag);
    }, { passive: false });

    doc.addEventListener("touchcancel", function () {
      cancelPress();
      if (selecting) {
        selecting = false;
        endSelectionDrag();
        return;
      }
      springBack();
    }, { passive: true });

    doc.addEventListener("touchend", function (e) {
      cancelPress();
      var axis = dragAxis;
      // Settle the range the finger drew: this is the emit that carries the
      // CFI and the overlapping highlight, which the drag emits leave out.
      if (selecting) {
        selecting = false;
        dragAxis = null;
        skipTap = false;
        endSelectionDrag();
        return;
      }
      stopDragRaf(axis === "x");
      dragAxis = null;
      if (dismissing) {
        dismissing = false;
        // A tap anywhere on the page puts the selection away — and does
        // nothing else. Turning the page or toggling the chrome on the same
        // tap would be two answers to one gesture.
        if (e.changedTouches && e.changedTouches.length) {
          var dt = e.changedTouches[0];
          if (Math.abs(stableX(dt) - sx) < 10 &&
              Math.abs(dt.clientY - sy) < 10 &&
              nowMs() - st < 700) {
            clearSelection();
          }
        }
        return;
      }
      if (skipTap) { skipTap = false; return; }
      if (!e.changedTouches || !e.changedTouches.length) return;
      var t = e.changedTouches[0];
      var endX = stableX(t);
      var endAt = nowMs();
      var dx = endX - sx;
      var dy = t.clientY - sy;
      var dt = endAt - st;

      if (axis === "x") {
        var c = pageContainer(), d = pageDelta();
        if (!c || !d) return;
        sampleVelocity(endX, endAt);
        var dir = 0;
        // Synthesized and real slow drags can reverse slightly at lift-off;
        // velocity only overrides position for an actual short flick.
        if (Math.abs(dragVx) > 0.45 && dt < 650) dir = dragVx < 0 ? 1 : -1;
        else if (Math.abs(dragIntentPx) > d * 0.3) dir = dragIntentPx < 0 ? 1 : -1;
        if (dir === 0) {
          animateOffsetTo(c, dragBase, dragPx, 0, 180);
          return;
        }
        var target = dragBase + dir * d;
        if (target < -0.5 || target > maxScroll(c) + 0.5) {
          // Preserve the resisted drag in the outgoing snapshot so it
          // continues smoothly instead of flashing back to centre first.
          // At the book's first/last page there is nothing to turn to —
          // settle the pulled page back instead of stranding the offset.
          if (!turnAcrossSection(dir)) {
            animateOffsetTo(c, dragBase, dragPx, 0, 180);
          }
          return;
        }
        var remaining = Math.abs((-dir * d) - dragPx) / d;
        animateOffsetTo(c, dragBase, dragPx, -dir * d, 150 + remaining * 120);
        return;
      }

      // RTL fallback: the classic swipe-at-release turn.
      if (rtl && Math.abs(dx) > 45 && Math.abs(dx) > Math.abs(dy) * 1.5) {
        if (dx < 0) next(); else prev();
        return;
      }

      // Otherwise a stationary tap. A highlight under it wins — the passage
      // is the more specific target — then the outer 20% gutters turn the
      // page, and anything else toggles the chrome.
      if (Math.abs(dx) < 10 && Math.abs(dy) < 10 && dt < 500) {
        var tap = hostPoint(t);
        var hit = annotationAtHostPoint(tap.x, tap.y);
        if (hit) {
          emitAnnotationTap(hit);
          return;
        }
        var w = window.innerWidth || 360;
        if (tap.x > w * 0.8) turnAnimated(1);
        else if (tap.x < w * 0.2) turnAnimated(-1);
        // A centre tap toggles the reader chrome for distraction-free reading.
        else emitToggleChrome();
      }
    }, { passive: true });
  }

  // Ask the host to toggle the reader chrome (top/bottom bars). The bars are
  // Dioxus-owned DOM; rather than mutate their class from here (the reconciler
  // could clobber it, and it splits state across two owners), we signal through
  // the same `__omnibusOn*` bridge the rest of the glue uses and let the host
  // flip a `chrome_hidden` signal — the single source of truth on both targets.
  function emitToggleChrome() {
    if (typeof window.__omnibusOnToggleChrome === "function") {
      window.__omnibusOnToggleChrome("");
    }
  }

  function setFontSize(px) {
    if (!rendition) return;
    rendition.themes.fontSize(px + "px");
    currentFontSize = px;
    applyMarkStyles();
  }

  // Page grounds, keyed by theme token. Mirrors the `themes.register` bodies
  // above — change both together.
  var HOST_GROUNDS = {
    light: "#fcfbfa",
    dark: "#201e1b",
    black: "#000000",
    sepia: "#ede4d0",
  };

  // Paint the *host* document the same ground as the page.
  //
  // The stage is inset from the safe areas, so the bands above and below it are
  // host document, not book. Left on the stylesheet's dark fallback they stay
  // dark behind a white page — which reads as a broken frame around the prose
  // now that the chrome floats over those bands instead of covering them.
  function applyHostGround(name) {
    var ground = HOST_GROUNDS[name];
    if (!ground || !document.documentElement) return;
    document.documentElement.style.setProperty("--page", ground);
  }

  function setTheme(name) {
    if (!rendition) return;
    rendition.themes.select(name);
    currentTheme = name;
    applyHostGround(name);
    applyMarkStyles();
    // Re-tint reader-owned colours in every rendered section for the new ground.
    try {
      rendition.getContents().forEach(function (c) {
        applyThemeColors(c.document);
      });
    } catch (e) {
      /* no rendered sections yet */
    }
  }

  function setFont(family) {
    if (!rendition) return;
    rendition.themes.font(family);
  }

  function setLineHeight(value) {
    if (!rendition) return;
    rendition.themes.override("line-height", value);
    currentLineHeight = Number(value) || currentLineHeight;
    applyMarkStyles();
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

  // How a mark is drawn, as one host-document rule.
  //
  // epub.js hands its `styles` to the mark's `<g>` as presentation
  // attributes. `fill` inherits down to the `<rect>`s it draws, but geometry
  // properties like `rx` don't — and the blend has to follow the reading
  // theme, which an attribute set once at insert time can't. A stylesheet
  // does both, and a CSS rule outranks a presentation attribute, so this is
  // the last word on either.
  //
  // The blend is the whole trick: `multiply` on a light page keeps black
  // type black under the colour, and `screen` on a dark one keeps white type
  // white. Use either on the wrong ground and the passage goes muddy.
  function markStyleCss(theme) {
    var light = theme === "light" || theme === "sepia";
    return (
      "#stage svg g[ref^='hl-']{" +
      "mix-blend-mode:" + (light ? "multiply" : "screen") + ";" +
      // Opacity on the *group*, not the fill: the rects below are grown
      // until they touch, and group opacity composites the whole set once,
      // so the overlaps don't darken into seams the way per-shape alpha
      // would.
      // The blend leaves the type alone at any strength — `screen` maps white
      // to white and `multiply` maps black to black — so this trades against
      // nothing but how loud the mark is.
      "opacity:" + (light ? "0.42" : "0.62") + ";}" +
      // Square. Rounded ends made a marked paragraph read as a stack of
      // separate pills rather than as one continuous run of marked text.
      "#stage svg g[ref^='hl-'] rect{" +
      // Every paint inside the group is fully opaque, and the group's own
      // `opacity` above does all the fading — that is the only way the mark
      // comes out one flat colour. epub.js merges its own defaults into the
      // style object (`Object.assign({fill:"yellow","fill-opacity":"0.3"},
      // styles)`), so without this the fill runs at 0.3 against whatever else
      // is painted at 1 and the mark bands into three tones: a bright rim, a
      // washed middle, and a dark seam where consecutive lines overlap.
      "fill-opacity:1;" +
      // Grown about its own centre, and only vertically — see `markScaleY`.
      "transform-box:fill-box;transform-origin:center;" +
      "transform:scaleY(" + markScaleY() + ");}" +
      // The note cue is a rule under the passage, so it neither blends nor
      // fades — it has to stay legible over its own highlight.
      "#stage svg g[ref='omn-note']{mix-blend-mode:normal;}" +
      // epub.js draws an underline as a `<line>` *plus* a bounding `<rect>`
      // with `fill:none`. That rect inherits the group's stroke, so the cue
      // came out as a box around the passage rather than a rule under it.
      "#stage svg g[ref='omn-note'] rect{stroke:none;}"
    );
  }

  // How much to grow a mark so consecutive lines meet, as a vertical scale.
  //
  // `getClientRects` measures the font box while the lines are set on the
  // line box, and the difference is the leading — which is exactly what
  // shows through as a stripe between the lines of a highlighted paragraph.
  // The 1.15 is roughly the share of the line box the glyphs occupy, so the
  // ratio is the leading the current line-height adds back.
  //
  // Scaling is what keeps the growth vertical. Growing the rect with a
  // stroke, as this used to, also pushed the mark half a stroke past the
  // first letter — so a highlight looked like it began on the space before
  // the word rather than on the word.
  function markScaleY() {
    var scale = currentLineHeight / 1.15;
    return Math.round(Math.min(1.9, Math.max(1, scale)) * 1000) / 1000;
  }

  function applyMarkStyles() {
    var style = document.getElementById("__omnibus_marks");
    if (!style) {
      style = document.createElement("style");
      style.id = "__omnibus_marks";
      document.head.appendChild(style);
    }
    style.textContent = markStyleCss(currentTheme);
  }

  // Solid fills — transparency is applied once via `fill-opacity` above.
  // Baking alpha into the colour too would multiply with it (0.3 x 0.3) and
  // render the highlight nearly invisible.
  var HIGHLIGHT_COLORS = {
    amber:  "rgb(245, 158, 11)",
    green:  "rgb(34, 197, 94)",
    blue:   "rgb(59, 130, 246)",
    rose:   "rgb(244, 63, 94)",
    violet: "rgb(139, 92, 246)",
  };

  // The cfiRange of a highlight the selection runs through, if any, so the
  // host can offer "Remove Highlight" the way Apple Books does when your
  // selection touches one.
  //
  // Compared as live DOM ranges rather than by parsing CFI strings: epub.js
  // can hand back the range for a stored annotation, and boundary-point
  // comparison is then exact and standard.
  function overlappingAnnotation(range) {
    if (!rendition || !rendition.annotations) return null;
    var store = rendition.annotations._annotations;
    if (!store || !range) return null;
    var keys = Object.keys(store);
    for (var i = 0; i < keys.length; i++) {
      var entry = store[keys[i]];
      if (!entry || entry.type !== "highlight") continue;
      try {
        var other = rendition.getRange(entry.cfiRange);
        if (!other) continue;
        // Overlap iff each range begins before the other ends.
        var otherStartsBeforeThisEnds =
          range.compareBoundaryPoints(Range.START_TO_END, other) < 0;
        var thisStartsBeforeOtherEnds =
          other.compareBoundaryPoints(Range.START_TO_END, range) < 0;
        if (otherStartsBeforeThisEnds && thisStartsBeforeOtherEnds) {
          return entry.cfiRange;
        }
      } catch (e) {
        /* a stored range in another section can't be resolved here */
      }
    }
    return null;
  }

  function windowOfRange(range) {
    try {
      var doc = range.startContainer.ownerDocument;
      return (doc && doc.defaultView) || null;
    } catch (e) {
      return null;
    }
  }

  // The highlight under a point, in host-window coordinates, with the same
  // per-line geometry a selection reports so the host anchors one menu the
  // same way for both.
  //
  // Hit-tested here rather than bound to the mark's own DOM listener: that
  // listener fires on `touchstart`, so a swipe that merely *began* on a
  // highlighted word opened a menu about it, and the tap also reached the
  // document as a page tap — two events for one touch, which the host could
  // only ever pair up by arrival time.
  function annotationAtHostPoint(x, y) {
    if (!rendition || !rendition.annotations) return null;
    var store = rendition.annotations._annotations;
    if (!store) return null;
    var keys = Object.keys(store);
    for (var i = 0; i < keys.length; i++) {
      var entry = store[keys[i]];
      if (!entry || entry.type !== "highlight") continue;
      // A string compare against the current spine item before the expensive
      // part: `getRange` parses a CFI and walks the DOM, and a well-marked
      // book has hundreds of these — paying that for every one on every tap
      // is what would put a stutter on turning the page.
      if (currentSectionBase &&
          String(entry.cfiRange).split("!")[0] !== currentSectionBase) {
        continue;
      }
      var range = null;
      try {
        range = rendition.getRange(entry.cfiRange);
      } catch (e) {
        continue;
      }
      var win = range && windowOfRange(range);
      if (!win) continue;
      var rects = lineRects(range, win);
      for (var k = 0; k < rects.length; k++) {
        var r = rects[k];
        // A little slop: a finger aiming at a line of type lands low, and
        // the mark is only as tall as the line box.
        if (x >= r.x - 4 && x <= r.x + r.width + 4 &&
            y >= r.y - 6 && y <= r.y + r.height + 6) {
          return { cfiRange: entry.cfiRange, rects: rects };
        }
      }
    }
    return null;
  }

  function emitAnnotationTap(hit) {
    if (typeof window.__omnibusOnAnnotationTap !== "function" || !hit) return;
    try {
      window.__omnibusOnAnnotationTap(JSON.stringify(hit));
    } catch (e) {
      /* ignore handler errors */
    }
  }

  function addAnnotation(cfiRange, color, hasNote) {
    if (!rendition) return;
    var fill = HIGHLIGHT_COLORS[color] || HIGHLIGHT_COLORS.amber;
    // No DOM listener on the mark — taps are hit-tested at touchend against
    // `annotationAtHostPoint`, which keeps one touch to one outcome.
    var onTap = function () {};
    // `fill` inherits from the <g> down to the rects marks-pane draws; the
    // stylesheet above does the rest.
    rendition.annotations.add(
      "highlight", cfiRange, {}, onTap, "hl-" + color, { fill: fill }
    );
    // A note is otherwise invisible on the page: without a cue, the only way
    // to find your own annotation is to remember where you left it.
    if (hasNote) {
      rendition.annotations.add(
        "underline", cfiRange, {}, onTap,
        "omn-note",
        { stroke: fill, "stroke-opacity": "0.95", "stroke-width": "2" }
      );
    }
  }

  function removeAnnotation(cfiRange) {
    if (!rendition) return;
    removeMark(cfiRange, "highlight");
    removeMark(cfiRange, "underline");
  }

  // `annotations.remove` throws when the mark isn't there, and the note
  // underline only exists for some highlights.
  function removeMark(cfiRange, type) {
    try {
      rendition.annotations.remove(cfiRange, type);
    } catch (e) {
      /* not present */
    }
  }

  function clearAnnotations() {
    if (!rendition || !rendition.annotations) return;
    var store = rendition.annotations._annotations;
    if (!store) return;
    var keys = Object.keys(store);
    for (var i = 0; i < keys.length; i++) {
      var entry = store[keys[i]];
      if (entry && (entry.type === "highlight" || entry.type === "underline")) {
        removeMark(entry.cfiRange, entry.type);
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
  // Where a TOC entry starts, in whole-book pages and percent.
  //
  // Only resolvable once the locations pass has run, which is why the toc is
  // emitted twice — once bare so the contents list is usable immediately, then
  // again with positions. Backs the page numbers in the contents list and the
  // "which chapter is this?" readout while scrubbing.
  function tocEntryPosition(href) {
    if (!sectionRanges || !book || !book.spine || !href) return null;
    var section;
    try {
      section = book.spine.get(String(href).split("#")[0]);
    } catch (e) {
      return null;
    }
    if (!section || typeof section.cfiBase !== "string") return null;
    var range = sectionRanges["epubcfi(" + section.cfiBase];
    if (!range) return null;
    var total = (book.locations && book.locations.total) || 0;
    return {
      page: range.first + 1,
      pct: total > 0 ? Math.round((range.first / total) * 100) : 0,
    };
  }

  function collectToc(items, level, out) {
    if (!items) return;
    for (var i = 0; i < items.length; i++) {
      var href = items[i].href || "";
      var entry = {
        label: (items[i].label || "").trim(),
        href: href,
        level: level,
      };
      var at = tocEntryPosition(href);
      if (at) {
        entry.page = at.page;
        entry.pct = at.pct;
      }
      out.push(entry);
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

  // epub.js's own content-link handling (wired in its Rendition constructor,
  // ahead of any hook this file registers) calls the *plain* `rendition
  // .display(href)` on every `<a href>` tap inside the book — bypassing both
  // corrections `display()` below applies (zero-size anchor resolution and
  // the fonts/theme settle-then-redisplay), which is why in-book links land
  // a page or two off while TOC/CFI navigation (which already goes through
  // `display()`) is accurate. epub.js emits a "linkClicked" event with the
  // fully-resolved in-book path *before* issuing that plain display, so we
  // piggyback on its own resolution (no need to reimplement href/base-tag
  // parsing) and re-run the same href through our corrected `display()`.
  // epub.js's own plain display() still fires first, but `rendition.display`
  // serializes calls through an internal queue and short-circuits a
  // still-in-flight one when a newer call arrives — the same "call display,
  // then call it again" pattern `displaySettled()` already relies on to
  // correct TOC navigation — so our corrected redisplay simply supersedes
  // the stale uncorrected one before the user sees it settle.
  function installContentLinkNav() {
    if (!rendition || !rendition.hooks || !rendition.hooks.content) return;
    rendition.hooks.content.register(function (contents) {
      if (!contents || typeof contents.on !== "function") return;
      contents.on("linkClicked", function (href) {
        if (!book || !book.path) return;
        display(book.path.relative(href));
      });
    });
  }

  // Navigate to a TOC href or a CFI (highlight / bookmark target).
  // Display `target`, then re-display it once the section's fonts/theme have
  // settled. The first pass measures the target's column in a freshly
  // rendered iframe whose metrics can still shift (webfont swap, theme
  // injection) — the reflow leaves the viewport pages past the target, and
  // the follow-up display corrects it against the final layout.
  // Wait for the active section's webfonts to settle, then re-display
  // `target` against the final layout. Returns a promise resolving once the
  // corrective redisplay completes, so callers can sequence on it.
  function redisplayWhenSettled(target) {
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
    // The web build's section iframe is sandboxed WITHOUT allow-scripts, and
    // a promise minted by a scripting-disabled realm never settles — awaiting
    // fonts.ready bare would hang forever there. Race it against a bounded
    // timer: where fonts.ready works (mobile WebView opts into scripts) the
    // correction stays font-accurate, and elsewhere the timer still
    // redisplays after the layout has settled.
    var bounded = Promise.race([
      ready,
      new Promise(function (res) {
        setTimeout(res, 1500);
      }),
    ]);
    return bounded
      .then(function () {
        return new Promise(function (res) {
          setTimeout(res, 80);
        });
      })
      .then(function () {
        if (rendition) return rendition.display(target);
      });
  }

  // Hide the stage while a navigation corrects itself.
  //
  // `displaySettled` lands a first pass, waits for fonts/injected CSS, then
  // re-displays — and that correction is a visible twitch on every chapter
  // jump. The book-open path already hides exactly this behind the host's
  // loading overlay; navigation had no equivalent.
  //
  // The stage's own opacity is the veil rather than a colour-filled overlay:
  // the web view is transparent and the host paints the reader ground beneath
  // it, so fading the stage out reveals the correct page colour in every
  // theme, with nothing to keep in sync.
  var settleFadeToken = 0;

  function beginSettleFade() {
    // Without scripts in the section iframe `fonts.ready` never settles, so
    // the correction only lands on `redisplayWhenSettled`'s 1.5s fail-safe —
    // far too long to hold a blank stage. Those builds keep the old visible
    // correction rather than trading a twitch for a stall.
    if (!scriptedContentAllowed) return function () {};

    var stage = rendition && rendition.manager && rendition.manager.container;
    if (!stage) return function () {};

    var token = ++settleFadeToken;
    // Out instantly — a fade-out would show the very frame we're hiding.
    stage.style.transition = "none";
    stage.style.opacity = "0";

    var reveal = function () {
      if (settleFadeToken !== token) return;
      settleFadeToken++;
      stage.style.transition = "opacity 180ms ease-out";
      stage.style.opacity = "1";
    };
    // Fail-open: a settle chain that never resolves must not leave the reader
    // showing a blank page forever.
    setTimeout(reveal, 2500);
    return reveal;
  }

  // Drop the veil outright (teardown, or a book swap mid-navigation) so a
  // later mount can never start with a hidden stage.
  function endSettleFade() {
    settleFadeToken++;
    var stage = rendition && rendition.manager && rendition.manager.container;
    if (!stage) return;
    stage.style.transition = "";
    stage.style.opacity = "";
  }

  function displaySettled(target) {
    if (!rendition) return;
    var reveal = beginSettleFade();
    rendition
      .display(target)
      .then(function () {
        return redisplayWhenSettled(target);
      })
      .then(reveal, function () {
        /* target may be gone after a teardown */
        reveal();
      });
  }

  // Saved positions are viewport-start CFIs — exact column boundaries — and
  // epub.js's column rounding can land `display(cfi)` one spread EARLY for a
  // boundary CFI (the target then sits just past the visible range). Compare
  // the target against the settled viewport and page once toward it. One
  // bounded step, best effort: anything further off is left where it landed.
  function nudgeToTarget(target) {
    try {
      var loc = rendition && rendition.currentLocation();
      if (!loc || !loc.start || !loc.end) return Promise.resolve();
      var cmp = new ePub.CFI();
      if (cmp.compare(target, loc.end.cfi) >= 0) {
        return rendition.next();
      }
      if (cmp.compare(target, loc.start.cfi) < 0) {
        return rendition.prev();
      }
    } catch (e) {
      /* CFI compare is best effort */
    }
    return Promise.resolve();
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

  // Jump to a fraction (0–1) of the whole book — the scrubber's contract.
  // Positions come from the locations pass, so this no-ops until page numbers
  // exist rather than guessing and landing somewhere arbitrary.
  function seek(fraction) {
    if (!rendition || !book || !book.locations || !locationsReady) return;
    var f = Number(fraction);
    if (!isFinite(f)) return;
    var cfi = book.locations.cfiFromPercentage(Math.max(0, Math.min(1, f)));
    if (cfi) display(cfi);
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
    // Native bridge first: WKWebView exposes no usable Web Share API, so the
    // mobile shell installs this shim and presents the OS share sheet from
    // Rust instead. Web builds never define it.
    if (typeof window.__omnibusOnShareText === "function") {
      try {
        window.__omnibusOnShareText(text);
      } catch (e) {
        /* ignore handler errors */
      }
      return;
    }
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

  // Native share sheet with the rendered PNG. The mobile shell's
  // `__omnibusOnShareImage` shim takes priority (WKWebView has no usable Web
  // Share API — Rust presents the real sheet); after that, Web Share where it
  // can take files, then a plain download (desktop browsers, older WebViews).
  function shareQuoteCard(json) {
    var r = renderQuoteCanvas(json);
    if (!r) return;
    if (typeof window.__omnibusOnShareImage === "function") {
      try {
        window.__omnibusOnShareImage(JSON.stringify({
          name: r.name,
          dataUrl: r.canvas.toDataURL("image/png"),
        }));
      } catch (e) {
        /* ignore handler errors */
      }
      return;
    }
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
    seek: seek,
    beginSelectionAt: beginSelectionAt,
    extendSelectionTo: extendSelectionTo,
    beginEdgeDrag: beginEdgeDrag,
    endSelectionDrag: endSelectionDrag,
    clearSelection: clearSelection,
    copyText: copyText,
    shareText: shareText,
    search: search,
    exportQuoteCard: exportQuoteCard,
    shareQuoteCard: shareQuoteCard,
    copyQuoteCardImage: copyQuoteCardImage,
    destroy: destroy,
  };
})();
