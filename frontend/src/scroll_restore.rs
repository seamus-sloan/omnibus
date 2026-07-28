//! Session-scoped scroll-position restoration across route navigations.
//! `dioxus_router` remounts a page fresh on back-nav, dropping its scroll
//! offset; a `window`-level JS singleton (installed via `document::eval`)
//! records `scrollY` per path and restores it once content has painted.
//! In-memory only; scrolling is on the document/window (web + mobile WebView).

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
///
/// Recording is frozen for the duration of a route transition: the `scroll`
/// events a transition fires are the browser clamping us against a
/// mid-navigation document (and re-applying its own history restoration), not
/// the reader moving, so recording them under the incoming path would
/// overwrite the very offset this restore is about to read.
#[cfg(any(feature = "web", feature = "mobile"))]
const RESTORE_JS: &str = r#"
(function () {
  if (!window.__omnibusScroll) {
    const positions = {};
    let raf = 0;
    // Path whose scroll offsets we trust. Null while a history navigation is
    // in flight; `restore` re-arms it once the incoming page has settled.
    let recordPath = location.pathname;
    const record = () => {
      raf = 0;
      if (recordPath === null || location.pathname !== recordPath) return;
      positions[recordPath] = window.scrollY;
    };
    window.addEventListener("scroll", () => {
      if (!raf) raf = requestAnimationFrame(record);
    }, { passive: true });
    window.addEventListener("popstate", () => { recordPath = null; });
    window.__omnibusScroll = {
      positions,
      restore(path) {
        const key = path || location.pathname;
        const target = positions[key];
        const rearm = () => { recordPath = key; };
        if (target == null || target <= 0) { rearm(); return; }
        let cancelled = false;
        const cancel = () => { cancelled = true; teardown(); };
        const teardown = () => {
          window.removeEventListener("wheel", cancel);
          window.removeEventListener("touchstart", cancel);
          window.removeEventListener("keydown", cancel);
          rearm();
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
