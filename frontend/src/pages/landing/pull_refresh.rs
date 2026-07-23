//! Pull-to-refresh for the mobile landing screen. A JS touch tracker owns
//! the drag visuals — styling the `.m-ptr` indicator imperatively, so
//! no per-frame WASM round trip — and fires one eval message when the pull
//! crosses the threshold; the Rust side awaits the forced first-page
//! refresh (`data::refresh_ebooks_first_page`) before settling the
//! indicator. Install/cleanup mirrors `use_mobile_edge_swipe_back`.

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
  var startY = 0, pull = 0, active = false;
  var THRESH = 70, MAX = 110;
  function el(){ return document.querySelector('.m-ptr'); }
  function top(){ return window.scrollY || document.documentElement.scrollTop || 0; }
  function onStart(e){
    if (window.__omnibusPtrBusy || !e.touches || e.touches.length !== 1 || top() > 0) { active = false; return; }
    startY = e.touches[0].clientY; pull = 0; active = true;
    var n = el(); if (n) n.style.transition = 'none';
  }
  function onMove(e){
    if (!active) return;
    var dy = e.touches[0].clientY - startY;
    pull = dy > 0 && top() <= 0 ? Math.min(MAX, Math.pow(dy, 0.85)) : 0;
    var n = el(); if (!n) return;
    var p = Math.min(1, pull / THRESH);
    n.style.opacity = String(p);
    n.style.transform = 'translate(-50%,' + (pull * 0.6) + 'px) rotate(' + Math.round(p * 180) + 'deg)';
  }
  function onEnd(){
    if (!active) return;
    active = false;
    var n = el(); if (!n) return;
    n.style.transition = '';
    if (pull >= THRESH) {
      window.__omnibusPtrBusy = true;
      window.__omnibusPtrT0 = Date.now();
      n.classList.add('is-refreshing');
      n.style.transform = 'translate(-50%, 44px)';
      try { dioxus.send(1); } catch (_e) {}
    } else {
      n.style.transform = ''; n.style.opacity = '';
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
  var wait = Math.max(0, 500 - (Date.now() - (window.__omnibusPtrT0 || 0)));
  setTimeout(function(){
    if (n) {
      n.classList.remove('is-refreshing');
      n.style.transform = '';
      n.style.opacity = '';
    }
    window.__omnibusPtrBusy = false;
  }, wait);
})();
"#;

/// Install the pull-to-refresh bridge. Reads `prefs` at trigger time so the
/// forced refetch always targets the currently active sort/filter page key.
pub(super) fn use_pull_to_refresh(server_url: String, prefs: Signal<ViewPrefs>) {
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
                data::refresh_ebooks_first_page(&url, p.sort_key, p.sort_dir, p.filters, PAGE_SIZE)
                    .await;
                let _ = dioxus::document::eval(SETTLE_JS);
            }
        }
    });
    use_drop(move || {
        dioxus::document::eval(&cleanup_js(id));
    });
}
