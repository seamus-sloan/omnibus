//! Pull-to-refresh for the mobile landing screen. A JS touch tracker moves
//! `.m-lib` and `.m-ptr` without per-frame WASM work, then asks Rust to refresh
//! the first page and settle the indicator. Install and cleanup mirror the
//! mobile edge-swipe hook so remounts cannot remove a newer listener.

use dioxus::prelude::*;
use omnibus_shared::ViewPrefs;

use crate::data;

use super::PAGE_SIZE;

/// Track a downward drag that starts at scroll-top: follow it with the
/// indicator (resistance-curved), and past the threshold hand off to Rust.
/// `id` tags the install so the unmount cleanup only removes its own
/// listener (see [`cleanup_js`]).
fn install_js(id: u64) -> String {
    const TEMPLATE: &str = r#"
(function(){
  var prev = window.__omnibusPtr;
  if (prev) {
    document.removeEventListener('touchstart', prev.onStart, true);
    document.removeEventListener('touchmove', prev.onMove, true);
    document.removeEventListener('touchend', prev.onEnd, true);
    document.removeEventListener('touchcancel', prev.onEnd, true);
  }
  // A replaced install's in-flight refresh can never settle (its Rust
  // future died with the unmount, so SETTLE_JS never runs) and the old
  // cleanup no-ops on the id check — reset the gate so this mount always
  // starts usable.
  window.__omnibusPtrBusy = false;
  var startY = 0, pull = 0, active = false;
  var THRESH = 70, MAX = 110, HOLD = 64;
  function el(){ return document.querySelector('.m-ptr'); }
  function lib(){ return document.querySelector('.m-lib'); }
  function top(){ return window.scrollY || document.documentElement.scrollTop || 0; }
  // The native pattern: the CONTENT follows the finger down and the spinner
  // sits in the gap it opens; past the threshold the content holds at HOLD
  // until the refresh settles, then springs back up.
  function settleLib(px){
    var l = lib(); if (!l) return;
    l.style.transition = 'transform .24s cubic-bezier(.2,.7,.3,1)';
    l.style.transform = px ? 'translateY(' + px + 'px)' : '';
  }
  function onStart(e){
    if (window.__omnibusPtrBusy || !e.touches || e.touches.length !== 1 || top() > 0) { active = false; return; }
    startY = e.touches[0].clientY; pull = 0; active = true;
    var n = el(); if (n) n.style.transition = 'none';
    // Inline transition:none beats the .m-lib-ready class transition, so
    // the drag tracks 1:1; the settle paths restore it for the return.
    var l = lib(); if (l) { l.style.transition = 'none'; l.style.willChange = 'transform'; }
  }
  function onMove(e){
    if (!active) return;
    var dy = e.touches[0].clientY - startY;
    pull = dy > 0 && top() <= 0 ? Math.min(MAX, Math.pow(dy, 0.85)) : 0;
    var p = Math.min(1, pull / THRESH);
    var n = el();
    if (n) {
      n.style.opacity = String(p);
      // The disc drifts only slightly — the content curtain moving over a
      // near-stationary spinner is what reads as the native gap.
      n.style.transform = 'translate(-50%,' + (pull * 0.1) + 'px) rotate(' + Math.round(p * 180) + 'deg)';
    }
    var l = lib();
    if (l) l.style.transform = pull ? 'translateY(' + pull + 'px)' : '';
  }
  function onEnd(){
    if (!active) return;
    active = false;
    var n = el();
    if (n) n.style.transition = '';
    if (n && pull >= THRESH) {
      window.__omnibusPtrBusy = true;
      window.__omnibusPtrT0 = Date.now();
      n.classList.add('is-refreshing');
      n.style.transform = 'translate(-50%, 4px)';
      settleLib(HOLD);
      try { dioxus.send(1); } catch (_e) {}
    } else {
      if (n) { n.style.transform = ''; n.style.opacity = ''; }
      settleLib(0);
    }
  }
  // Passive capture listeners: coordinates only, never preventDefault —
  // taps, scrolls, and the edge-swipe-back tracker stay untouched.
  document.addEventListener('touchstart', onStart, { capture: true, passive: true });
  document.addEventListener('touchmove', onMove, { capture: true, passive: true });
  document.addEventListener('touchend', onEnd, { capture: true, passive: true });
  document.addEventListener('touchcancel', onEnd, { capture: true, passive: true });
  window.__omnibusPtr = { onStart: onStart, onMove: onMove, onEnd: onEnd, id: __ID__ };
})();
"#;
    TEMPLATE.replace("__ID__", &id.to_string())
}

/// Remove this screen's tracker on unmount — only when it's still the
/// installed one (`id` match), so a remount that already rebound survives.
fn cleanup_js(id: u64) -> String {
    const TEMPLATE: &str = r#"
(function(){
  var s = window.__omnibusPtr;
  if (s && s.id === __ID__) {
    document.removeEventListener('touchstart', s.onStart, true);
    document.removeEventListener('touchmove', s.onMove, true);
    document.removeEventListener('touchend', s.onEnd, true);
    document.removeEventListener('touchcancel', s.onEnd, true);
    window.__omnibusPtr = null;
    window.__omnibusPtrBusy = false;
  }
})();
"#;
    TEMPLATE.replace("__ID__", &id.to_string())
}

/// Settle the indicator once the refresh lands, holding it visible long
/// enough (500ms floor from trigger time) that a fast server round trip
/// doesn't read as a flicker.
const SETTLE_JS: &str = r#"
(function(){
  var n = document.querySelector('.m-ptr');
  var l = document.querySelector('.m-lib');
  var wait = Math.max(0, 500 - (Date.now() - (window.__omnibusPtrT0 || 0)));
  setTimeout(function(){
    if (n) {
      n.classList.remove('is-refreshing');
      n.style.transform = '';
      n.style.opacity = '';
    }
    if (l) {
      // Spring the held content back up, then hand transitions back to
      // the .m-lib-ready class and drop the gesture's layer promotion.
      l.style.transition = 'transform .24s cubic-bezier(.2,.7,.3,1)';
      l.style.transform = '';
      setTimeout(function(){
        l.style.transition = '';
        l.style.willChange = '';
      }, 300);
    }
    window.__omnibusPtrBusy = false;
  }, wait);
})();
"#;

/// Install the pull-to-refresh bridge. Reads `prefs` at trigger time so the
/// forced refetch always targets the currently active sort/filter page key.
pub(super) fn use_pull_to_refresh(server_url: String, prefs: Signal<ViewPrefs>) {
    // Read at trigger time (like `prefs`) so the forced refetch lands in the
    // same SWR key the landing is actively reading — exclusion included.
    let viewer = crate::use_current_user_summary();
    // Per-mount id so the unmount cleanup only removes its own listener.
    let id = use_hook(|| {
        static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    });
    let mut eval = use_hook(|| dioxus::document::eval(&install_js(id)));
    use_future(move || {
        let url = server_url.clone();
        async move {
            // Loop ends when the channel closes (this screen unmounted).
            while eval.recv::<i32>().await.is_ok() {
                let p = prefs.peek().clone();
                let exclude = viewer
                    .peek()
                    .as_ref()
                    .map(|u| {
                        let mut h = u.hidden_formats.clone();
                        h.sort();
                        h
                    })
                    .unwrap_or_default();
                data::refresh_ebooks_first_page(
                    &url, p.sort_key, p.sort_dir, p.filters, exclude, PAGE_SIZE,
                )
                .await;
                let _ = dioxus::document::eval(SETTLE_JS);
            }
        }
    });
    use_drop(move || {
        dioxus::document::eval(&cleanup_js(id));
    });
}
