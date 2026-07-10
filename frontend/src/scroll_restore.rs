//! Session-scoped scroll-position restoration across route navigations.
//!
//! `dioxus_router` has no keep-alive: navigating to a detail route unmounts
//! the index page, and going back re-mounts it fresh, dropping its scroll
//! offset. This module keeps a per-path scroll map on a `window`-level JS
//! singleton (the SPA `window` survives route changes, so the map does too)
//! and restores the offset once a page's content has painted. In-memory only —
//! restoring after a full reload / cold start is a non-goal.
//!
//! The document/`window` is the scroll container app-wide (no inner `overflow`
//! scroller), so `window.scrollY` / `window.scrollTo` is the axis on both web
//! and the mobile WebView. Interop goes through `dioxus::document::eval`, the
//! shared web+mobile seam — mirroring the load-more `IntersectionObserver` in
//! `pages/landing/effects.rs`.

use dioxus::prelude::*;

/// Ensure the `window.__omnibusScroll` manager is installed, then restore the
/// saved offset for the current path. Idempotent — the installer guards on the
/// singleton so repeated calls only re-trigger a restore.
///
/// Restore retries across animation frames: a page's content arrives after
/// mount (async fetch, images, and — on the paginated library — further pages
/// streamed in by the load-more observer as we scroll toward the target), so a
/// single `scrollTo` would land short. The loop keeps nudging toward the
/// captured target until it's reached, the user scrolls (cancelling), the
/// document stops growing, or a time budget elapses.
#[cfg(any(feature = "web", feature = "mobile"))]
const RESTORE_JS: &str = r#"
(function () {
  if (!window.__omnibusScroll) {
    const positions = {};
    let raf = 0;
    const record = () => { raf = 0; positions[location.pathname] = window.scrollY; };
    window.addEventListener("scroll", () => {
      if (!raf) raf = requestAnimationFrame(record);
    }, { passive: true });
    window.__omnibusScroll = {
      positions,
      restore(path) {
        const key = path || location.pathname;
        const target = positions[key];
        if (target == null || target <= 0) return;
        let cancelled = false;
        const cancel = () => { cancelled = true; teardown(); };
        const teardown = () => {
          window.removeEventListener("wheel", cancel);
          window.removeEventListener("touchstart", cancel);
          window.removeEventListener("keydown", cancel);
        };
        // A real scroll intent from the user wins over restoration.
        window.addEventListener("wheel", cancel, { passive: true });
        window.addEventListener("touchstart", cancel, { passive: true });
        window.addEventListener("keydown", cancel);
        const start = performance.now();
        let lastMax = -1;
        let lastGrewAt = start;
        const step = (now) => {
          if (cancelled) return;
          window.scrollTo(0, target);
          if (Math.abs(window.scrollY - target) < 2) { teardown(); return; }
          const max = document.documentElement.scrollHeight - window.innerHeight;
          if (max > lastMax) { lastMax = max; lastGrewAt = now; }
          // Give up if the document has stopped growing while still too short
          // to reach the target (no more pages coming), or after a hard cap.
          const stalled = max < target && now - lastGrewAt > 1500;
          if (stalled || now - start > 6000) { teardown(); return; }
          requestAnimationFrame(step);
        };
        requestAnimationFrame(step);
      },
    };
  }
  window.__omnibusScroll.restore(location.pathname);
})();
"#;

/// Restore scroll for the current route once `ready` first becomes `true`
/// (i.e. the page's primary content has loaded and the document has height to
/// scroll into). Fires exactly once per mount.
///
/// Call this unconditionally, before any early `return` in the component, so
/// the hook order stays stable across renders.
pub fn use_scroll_restore(ready: Memo<bool>) {
    let mut done = use_signal(|| false);
    use_effect(move || {
        if *done.peek() || !ready() {
            return;
        }
        done.set(true);
        run_restore();
    });
}

#[cfg(any(feature = "web", feature = "mobile"))]
fn run_restore() {
    let _ = dioxus::document::eval(RESTORE_JS);
}

// SSR / server-only build: effects don't run during SSR and there's no DOM to
// drive, so this is inert. Kept so the hook compiles on every target.
#[cfg(not(any(feature = "web", feature = "mobile")))]
fn run_restore() {}
