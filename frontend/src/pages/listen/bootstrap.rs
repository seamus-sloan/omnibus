//! Web-only `OmnibusAudio` bootstrap: installs the JS control surface on
//! `window`, wires Rust callbacks for time / duration / play / pause, then
//! fetches the manifest and branches on `mode` (direct vs hls) to seed
//! playback. Extracted from `BookListenPage` so the parent stays under the
//! 150-line component cap; everything here is web-feature gated.

#![cfg(feature = "web")]

use dioxus::prelude::*;
use wasm_bindgen::prelude::*;

use super::helpers::{post_audio_progress, HLS_JS};
use crate::data;

/// Owns the `Closure`s wired into `window.__omnibusOn*` callbacks. Held
/// across re-renders by `use_hook` so the closures outlive the effect
/// closure that registers them.
type JsCallbackHolder = std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(f64)>>>>;

// The functions below are the Rust-side decision surface, extracted so they
// can be unit-tested without a browser. Everything else in this module is a JS
// interop seam that needs a WASM runtime: `register_js_callbacks` writes
// closures into `window.*` properties, `inject_hls_script` /
// `install_control_surface` call `eval()`, and `run_manifest_init` runs the
// async manifest fetch + `/status` poll loop.

/// Select the resume position: prefer the server-authoritative value when
/// available, fall back to the locally cached initial position.
fn resolve_resume_pos(server_pos: Option<f64>, local_pos: f64) -> f64 {
    server_pos.unwrap_or(local_pos)
}

/// Extract `file_id` from the current URL's query string (`?file_id=N`),
/// targeting a specific `book_files` row for multi-file audiobooks.
fn parse_file_id_from_url() -> Option<i64> {
    let search = web_sys::window()?.location().search().ok()?;
    let params = web_sys::UrlSearchParams::new_with_str(&search).ok()?;
    params.get("file_id")?.parse().ok()
}

/// App-root playback driver. Installs the `window.OmnibusAudio` shim and
/// drives manifest-based playback off the app-wide [`crate::PlaybackState`].
///
/// Called once from [`crate::App`] (so the audio element and all signals
/// outlive any single route). The inner `use_effect` reacts to
/// `playback.uuid`: setting it (the listen page on mount, or a dock dismiss
/// that clears it) loads, swaps, or tears down playback. Because the signals
/// now outlive the page, every book swap must reset the prior book's state
/// before installing fresh callbacks.
pub(crate) fn install_audio_bootstrap(playback: crate::PlaybackState) {
    let cb_holder: JsCallbackHolder =
        use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

    use_effect(move || {
        // The only reactive dependency — re-run when the active book changes.
        let Some(uuid) = playback.uuid.read().clone() else {
            // Dismissed via the dock × button. The handler already stopped
            // the element; clear the book so the dock hides everywhere.
            let mut book = playback.book;
            book.set(None);
            return;
        };
        boot_new_book(&cb_holder, &uuid, playback);
    });
}

/// Reset the prior book's state and install fresh JS + Rust callbacks for
/// the newly-active `uuid`. Spawned tasks (metadata fetch + manifest init)
/// guard their writes on the live `playback.uuid` so a stale fetch can't
/// clobber a subsequent book swap.
fn boot_new_book(cb_holder: &JsCallbackHolder, uuid: &str, playback: crate::PlaybackState) {
    // Signals outlive the page, so a swap must reset stale per-book state
    // before installing fresh callbacks — otherwise the previous book's
    // metadata/position/chapters leak under the new uuid until the async
    // fetches land. Clear the cross-book fields (book/error/chapters)
    // synchronously here, not just the playback scalars.
    reset_per_book_signals(&playback, uuid);

    let initial_position = crate::audiobook_progress::load(uuid).unwrap_or(0.0);
    let initial_rate = crate::audiobook_progress::load_rate(uuid);

    register_js_callbacks(
        cb_holder,
        uuid.to_string(),
        playback.duration,
        playback.elapsed,
        playback.playing,
        playback.playback_failed,
    );
    inject_hls_script();
    install_control_surface(uuid, initial_rate);

    // Stale-task guard: the user can switch books while these async tasks
    // are in flight. Each task captures the uuid it was spawned for and
    // checks the live `playback.uuid` before writing any shared signal, so
    // a stale fetch / status-poll can't clobber the new book's state.
    let guard = playback.uuid;

    spawn_book_metadata_fetch(uuid.to_string(), guard, playback);
    spawn_manifest_init(uuid.to_string(), initial_position, playback, guard);
}

/// Synchronously clear playback scalars and cross-book fields before the
/// next book's async fetches land, so prior metadata/chapters don't leak.
fn reset_per_book_signals(playback: &crate::PlaybackState, uuid: &str) {
    let mut duration = playback.duration;
    let mut elapsed = playback.elapsed;
    let mut playing = playback.playing;
    let mut hls_ready = playback.hls_ready;
    let mut playback_failed = playback.playback_failed;
    let mut rate = playback.rate;
    let mut book = playback.book;
    let mut error = playback.error;
    let mut chapters = playback.chapters;

    duration.set(0.0_f64);
    elapsed.set(0.0_f64);
    playing.set(false);
    hls_ready.set(false);
    playback_failed.set(false);
    book.set(None);
    error.set(None);
    chapters.set(Vec::new());
    rate.set(crate::audiobook_progress::load_rate(uuid));
}

/// Fetch the book metadata into the shared context so both the full
/// player and the mini-dock can render cover + title.
fn spawn_book_metadata_fetch(
    uuid: String,
    guard: Signal<Option<String>>,
    playback: crate::PlaybackState,
) {
    let mut book = playback.book;
    let mut loading = playback.loading;
    let mut error = playback.error;
    spawn(async move {
        loading.set(true);
        let result = data::get_ebook("", &uuid).await;
        if guard.peek().as_deref() != Some(uuid.as_str()) {
            return; // a newer book was selected while we awaited
        }
        match result {
            Ok(b) => {
                book.set(b);
                error.set(None);
            }
            Err(e) => error.set(Some(e.to_string())),
        }
        loading.set(false);
    });
}

/// Kick off the manifest fetch + branch on `mode` (direct/HLS/none). The
/// active book's `file_id` (multi-file books) rides on the listen route's
/// query string; read it here since the App-level driver no longer
/// receives it as a param.
fn spawn_manifest_init(
    uuid: String,
    initial_position: f64,
    playback: crate::PlaybackState,
    guard: Signal<Option<String>>,
) {
    let fid = parse_file_id_from_url();
    let hls_ready = playback.hls_ready;
    let playback_failed = playback.playback_failed;
    let chapters = playback.chapters;
    spawn(async move {
        run_manifest_init(
            uuid,
            fid,
            initial_position,
            hls_ready,
            playback_failed,
            chapters,
            guard,
        )
        .await;
    });
}

/// Wire the five Rust-side closures that JS calls back into:
/// `__omnibusOnAudioTime`, `__omnibusOnAudioDuration`, `__omnibusOnAudioPlay`,
/// `__omnibusOnAudioPause`, `__omnibusOnInitTimeout`. Registered before the
/// JS bootstrap so a fast `loadedmetadata` always finds them.
fn register_js_callbacks(
    cb_holder: &JsCallbackHolder,
    uuid_cb: String,
    mut duration: Signal<f64>,
    mut elapsed: Signal<f64>,
    mut playing: Signal<bool>,
    mut playback_failed: Signal<bool>,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let uuid_for_save = uuid_cb.clone();
    let mut last_saved = 0.0_f64;
    let on_time = Closure::<dyn FnMut(f64)>::new(move |secs: f64| {
        elapsed.set(secs);
        if (secs - last_saved).abs() < 5.0 {
            return;
        }
        last_saved = secs;
        crate::audiobook_progress::save(&uuid_for_save, secs);
        post_audio_progress(uuid_for_save.clone(), secs);
    });
    let on_duration = Closure::<dyn FnMut(f64)>::new(move |d: f64| {
        duration.set(d);
    });
    let on_play = Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        playing.set(true);
    });
    let uuid_for_pause = uuid_cb;
    let on_pause = Closure::<dyn FnMut(f64)>::new(move |secs: f64| {
        playing.set(false);
        crate::audiobook_progress::save(&uuid_for_pause, secs);
        post_audio_progress(uuid_for_pause.clone(), secs);
    });
    // Fired from the init-poll's `n >= 200` branch when
    // `window.OmnibusAudio` never appears (mount loop gave
    // up, JS eval failed, vendored asset 404'd, …). Mirrors
    // the `reader.rs::__omnibusOnStatus("error")` shape so
    // the UI surfaces a real failure instead of stalling
    // on a perpetually-not-ready state.
    let on_init_timeout = Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        playback_failed.set(true);
    });

    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioTime"),
        on_time.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioDuration"),
        on_duration.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioPlay"),
        on_play.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioPause"),
        on_pause.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnInitTimeout"),
        on_init_timeout.as_ref().unchecked_ref(),
    );
    *cb_holder.borrow_mut() = vec![on_time, on_duration, on_play, on_pause, on_init_timeout];
}

/// Inject the vendored hls.js bundle once. The script tag is idempotent
/// because the browser caches it by URL.
fn inject_hls_script() {
    let hls_src = serde_json::to_string(&HLS_JS.to_string()).unwrap_or_else(|_| "\"\"".into());
    let inject_js = format!(
        r#"(function(){{
            if (window.Hls) return;
            var s = document.createElement('script');
            s.src = {hls_src};
            s.async = true;
            document.head.appendChild(s);
        }})();"#
    );
    let _ = dioxus::document::eval(&inject_js);
}

/// Install the `window.OmnibusAudio` control surface immediately so
/// the transport buttons are wired even before `initDirect` / `initHls`
/// attaches. The two init paths are responsible for setting their own
/// initial position (per-part for direct, absolute for hls) via one-shot
/// `loadedmetadata` listeners.
fn install_control_surface(uuid: &str, initial_rate: f64) {
    let rate_lit = serde_json::to_string(&initial_rate).unwrap_or_else(|_| "1".into());
    let uuid_lit = serde_json::to_string(uuid).unwrap_or_else(|_| "\"\"".into());
    let _ = dioxus::document::eval(&control_surface_js(&rate_lit, &uuid_lit));
}

/// Build the `window.OmnibusAudio` IIFE script (one self-contained JS module).
///
/// Composes four sub-segments: a DOM-reset/listener block (with the
/// initial playback rate interpolated in), the in-line OmnibusAudio
/// object scaffold (controls + state + the book uuid), and the two pure
/// JS init methods for direct-play and HLS. Mismatched brace escapes in
/// the `format!` args would silently break audio playback, so each pure
/// JS segment lives in its own helper as a raw `&'static str` (literal
/// `{`/`}`, no escaping required) and only `control_surface_js` itself
/// uses `format!` for the two interpolation points.
fn control_surface_js(rate_lit: &str, uuid_lit: &str) -> String {
    format!(
        r#"
(function(){{
{dom_reset}
    el.playbackRate = {rate_lit};
{listeners}

    window.OmnibusAudio = {{
{transport}
{direct_init}

{hls_init}

      _uuid: {uuid_lit},
    }};
  }}
  mount();
}})();
"#,
        dom_reset = dom_reset_js(),
        listeners = listeners_js(),
        transport = transport_controls_js(),
        direct_init = direct_play_init_js(),
        hls_init = hls_init_js(),
    )
}

/// OmnibusAudio object scaffold: mode + direct-mode state plus the
/// transport-control methods (play/pause/toggle/setRate/stop/seek/skip).
/// Pure JS with no Rust interpolation, so it lives as a raw `&'static
/// str` with literal braces.
fn transport_controls_js() -> &'static str {
    r#"      // Playback mode. Set by initDirect / initHls; null before either fires.
      _mode: null,
      // Direct-mode state — null in HLS mode.
      _parts: null,
      _index: 0,
      _cumOffsets: [],
      _totalDuration: 0,

      play:    function(){ var p = el.play(); if (p && p.catch) p.catch(function(){}); },
      pause:   function(){ el.pause(); },
      toggle:  function(){ if (el.paused) { this.play(); } else { this.pause(); } },
      setRate: function(r){ try { el.playbackRate = r; } catch(_) {} },
      // Sleep-timer volume fade. Clamps to [0,1]; the Rust countdown ramps
      // this down over the final seconds and restores 1.0 on cancel/expiry.
      setVolume: function(v){ try { el.volume = Math.max(0, Math.min(1, v)); } catch(_) {} },

      // Hard stop for the dock's dismiss: pause, drop the source so a
      // media-key resume can't restart it, and reset direct-mode state.
      stop:    function(){
        try { el.pause(); } catch(_) {}
        try { el.removeAttribute('src'); el.load(); } catch(_) {}
        this._mode = null; this._parts = null; this._index = 0;
        this._cumOffsets = []; this._totalDuration = 0;
      },

      // Seek to an absolute (cross-part) second offset. For direct mode
      // this finds the target part by cumulative duration and switches
      // `el.src` if it differs from the current part, preserving the
      // play/pause state across the swap.
      seek: function(absSeconds){
        var s = Math.max(0, absSeconds || 0);
        if (this._mode === 'direct' && this._parts) {
          var i = 0;
          while (i < this._cumOffsets.length - 1 && s >= this._cumOffsets[i + 1]) i++;
          var local = s - this._cumOffsets[i];
          if (i !== this._index) {
            var wasPlaying = !el.paused;
            this._index = i;
            var onMeta = function(){
              el.removeEventListener('loadedmetadata', onMeta);
              try { el.currentTime = local; } catch(_) {}
              if (wasPlaying) {
                var p = el.play(); if (p && p.catch) p.catch(function(){});
              }
            };
            el.addEventListener('loadedmetadata', onMeta);
            el.src = this._parts[i].url;
            el.load();
          } else {
            try { el.currentTime = local; } catch(_) {}
          }
        } else {
          try { el.currentTime = s; } catch(_) {}
        }
      },

      // Relative skip (+30 / -30). Computed in absolute terms so a skip
      // straddling a part boundary works correctly.
      skip: function(d){ this.seek(absTime() + d); },
"#
}

/// IIFE prologue + element reset: clears any stale `window.OmnibusAudio`
/// captured by an SPA-nav from the previous visit, polls for the audio
/// element, and resets its src/preload. Stops just before
/// `el.playbackRate` so the composer can interpolate the initial rate.
fn dom_reset_js() -> &'static str {
    r#"  // SPA-nav from another page leaves a stale `window.OmnibusAudio` from
  // the previous visit, captured in a closure over a now-detached
  // `<audio>` element. The init poll below sees that stale object,
  // calls `initDirect` on it, and the visible audio element never gets
  // a src — the scrub bar reads 0:00 until a full reload. Clearing
  // here forces the init poll to wait for the fresh install.
  try { var _prev = window.OmnibusAudio; if (_prev) { _prev._stale = true; } } catch(_) {}
  window.OmnibusAudio = null;
  // Wait for the audio element to appear in the DOM.
  var n = 0;
  function mount(){
    var el = document.getElementById('omnibus-audio');
    if (!el) { if (n++ < 200) { return setTimeout(mount, 50); } else { return; } }
    // Reset the element so leftover src / preloading from a prior mount
    // doesn't keep streaming once we swap modes.
    try { el.pause(); } catch(_) {}
    el.removeAttribute('src');
    el.preload = 'auto';"#
}

/// `absTime()` helper and the audio-element event listeners
/// (loadedmetadata / timeupdate / play / pause / ended). Pure JS with no
/// Rust interpolation, so it lives as a raw `&'static str` (literal
/// braces, no escaping).
fn listeners_js() -> &'static str {
    r#"
    // Helper: absolute seconds across the whole book. For direct mode
    // this adds the current part's cumulative offset; for HLS the audio
    // element's `currentTime` already IS absolute (one continuous
    // timeline).
    function absTime() {
      var oa = window.OmnibusAudio;
      if (oa && oa._mode === 'direct' && oa._parts) {
        return (oa._cumOffsets[oa._index] || 0) + (el.currentTime || 0);
      }
      return el.currentTime || 0;
    }

    el.addEventListener('loadedmetadata', function(){
      // Initial seek is the job of init{Direct,Hls} via their own
      // one-shot loadedmetadata listeners — this listener only reports
      // duration. For direct mode we always report the book-level total
      // so a part change does not collapse the scrub bar to per-part.
      var oa = window.OmnibusAudio;
      if (window.__omnibusOnAudioDuration) {
        var d = (oa && oa._mode === 'direct' && oa._totalDuration > 0)
          ? oa._totalDuration
          : (el.duration || 0);
        window.__omnibusOnAudioDuration(d);
      }
    });
    el.addEventListener('timeupdate', function(){
      if (window.__omnibusOnAudioTime) {
        window.__omnibusOnAudioTime(absTime());
      }
    });
    el.addEventListener('play', function(){
      if (window.__omnibusOnAudioPlay) {
        window.__omnibusOnAudioPlay(absTime());
      }
    });
    el.addEventListener('pause', function(){
      if (window.__omnibusOnAudioPause) {
        window.__omnibusOnAudioPause(absTime());
      }
    });
    // Cross-part advance — direct mode only. HLS treats the whole
    // book as one continuous stream so `ended` only fires at the
    // actual end (which we leave as-is so the UI naturally stops).
    el.addEventListener('ended', function(){
      var oa = window.OmnibusAudio;
      if (!oa || oa._mode !== 'direct' || !oa._parts) return;
      if (oa._index + 1 < oa._parts.length) {
        oa._index += 1;
        el.src = oa._parts[oa._index].url;
        el.load();
        var p = el.play(); if (p && p.catch) p.catch(function(){});
      }
    });"#
}

/// JS body for `initDirect: function(parts, initialPositionAbs)` — picks
/// the starting part by cumulative offset and wires a one-shot
/// `loadedmetadata` listener to seek into it. Pure JS; lives as a raw
/// `&'static str` with literal braces.
fn direct_play_init_js() -> &'static str {
    r#"      // Direct-play init: parts is the array from the manifest endpoint
      // (`[{ordinal, url, duration_seconds, mime}]`), initialPositionAbs
      // is the absolute resume position in seconds.
      initDirect: function(parts, initialPositionAbs){
        this._mode = 'direct';
        this._parts = parts;
        var acc = 0;
        this._cumOffsets = [];
        for (var i = 0; i < parts.length; i++) {
          this._cumOffsets.push(acc);
          acc += parts[i].duration_seconds || 0;
        }
        this._totalDuration = acc;
        // Push the total duration up to Rust eagerly so the scrub bar
        // gets the right max before the first part's metadata loads.
        if (window.__omnibusOnAudioDuration) {
          window.__omnibusOnAudioDuration(this._totalDuration);
        }
        // Pick the starting part for the resume position.
        var s = Math.max(0, initialPositionAbs || 0);
        var idx = 0;
        while (idx < this._cumOffsets.length - 1 && s >= this._cumOffsets[idx + 1]) idx++;
        this._index = idx;
        var local = s - this._cumOffsets[idx];
        var onMeta = function(){
          el.removeEventListener('loadedmetadata', onMeta);
          try { el.currentTime = local; } catch(_) {}
        };
        el.addEventListener('loadedmetadata', onMeta);
        el.src = parts[idx].url;
        el.load();
      },"#
}

/// JS body for `initHls: function(url, initialPositionAbs)` — legacy
/// fallback for codecs the browser cannot play natively. Tries Hls.js,
/// then Safari/iOS native HLS, then warns. Pure JS; lives as a raw
/// `&'static str` with literal braces.
fn hls_init_js() -> &'static str {
    r#"      // HLS init: legacy fallback path for codecs the browser does not
      // play natively. `initialPositionAbs` is optional — Rust passes
      // `null` to skip the seek.
      initHls: function(url, initialPositionAbs){
        this._mode = 'hls';
        if (typeof Hls !== 'undefined' && Hls.isSupported()) {
          var hls = new Hls();
          hls.loadSource(url);
          hls.attachMedia(el);
          hls.on(Hls.Events.ERROR, function(_, d) {
            if (d.fatal && window.__omnibusOnAudioPause) {
              window.__omnibusOnAudioPause(el.currentTime || 0);
            }
          });
        } else if (el.canPlayType('application/vnd.apple.mpegurl')) {
          // Safari / iOS native HLS.
          el.src = url;
          el.load();
        } else {
          console.warn('OmnibusAudio: no HLS support in this browser');
        }
        if (typeof initialPositionAbs === 'number' && initialPositionAbs > 0) {
          var onMeta = function(){
            el.removeEventListener('loadedmetadata', onMeta);
            try { el.currentTime = Math.max(0, initialPositionAbs); } catch(_) {}
          };
          el.addEventListener('loadedmetadata', onMeta);
        }
      },"#
}

/// Fetch `/api/audiobooks/{uuid}/manifest` (with optional `?file_id`)
/// and either:
/// * **Direct mode** — call `initDirect` with the parts list and flip
///   `hls_ready` true (instant playback for m4b/m4a/mp3/aac).
/// * **HLS mode** — poll `/status` until `ready` (call `initHls` + flip
///   `hls_ready`) or `failed` (flip `playback_failed`).
/// * **No manifest** — show the same failure overlay as a terminal HLS
///   transcode failure.
async fn run_manifest_init(
    uuid_for_fetch: String,
    file_id: Option<i64>,
    initial_position: f64,
    hls_ready: Signal<bool>,
    mut playback_failed: Signal<bool>,
    chapters_sig: Signal<Vec<omnibus_shared::ChapterInfo>>,
    uuid_guard: Signal<Option<String>>,
) {
    // True only while `uuid_for_fetch` is still the active book. Checked before
    // every shared-signal write so a stale task (user switched books mid-fetch
    // or mid-`/status`-poll) can't clobber the new book's state.
    let is_current = || uuid_guard.peek().as_deref() == Some(uuid_for_fetch.as_str());
    // Reconcile resume position with the server upfront so
    // both init paths see the same starting point.
    let server_pos = data::get_progress("", &uuid_for_fetch, omnibus_shared::ProgressFormat::Audio)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.audio_position_seconds);
    let resume_pos = resolve_resume_pos(server_pos, initial_position);
    let pos_lit = serde_json::to_string(&resume_pos).unwrap_or_else(|_| "0".into());

    let manifest = fetch_manifest(&uuid_for_fetch, file_id).await;

    // A newer book may have been selected while the manifest was in flight.
    // The Direct + None arms below are synchronous, so this single guard covers
    // them; the HLS arm re-checks each poll iteration.
    if !is_current() {
        return;
    }

    match manifest {
        Some(omnibus_shared::AudiobookManifest::Direct {
            parts, chapters, ..
        }) => init_direct_play(parts, chapters, &pos_lit, chapters_sig, hls_ready),
        Some(omnibus_shared::AudiobookManifest::Hls { playlist_url }) => {
            init_hls(
                &uuid_for_fetch,
                &playlist_url,
                &pos_lit,
                is_current,
                hls_ready,
                playback_failed,
            )
            .await;
        }
        None => {
            // Manifest unreachable (network failure, 5xx,
            // 404 between settings save and reindex). Show
            // the same failure overlay as a terminal HLS
            // transcode failure — a manual refresh is the
            // recovery path either way.
            playback_failed.set(true);
        }
    }
}

/// GET the audiobook manifest (`?file_id=N` for multi-file books) and
/// decode it. Returns `None` on network error or non-200.
async fn fetch_manifest(
    uuid: &str,
    file_id: Option<i64>,
) -> Option<omnibus_shared::AudiobookManifest> {
    let manifest_url = match file_id {
        Some(fid) => format!("/api/audiobooks/{uuid}/manifest?file_id={fid}"),
        None => format!("/api/audiobooks/{uuid}/manifest"),
    };
    match gloo_net::http::Request::get(&manifest_url).send().await {
        Ok(resp) if resp.status() == 200 => resp.json().await.ok(),
        _ => None,
    }
}

/// Direct-mode arm: hand the part list to the JS `initDirect` shim,
/// publish the chapter map, and flip `hls_ready` true. Synchronous —
/// the caller's `is_current` guard covered the only `await`.
fn init_direct_play(
    parts: Vec<omnibus_shared::ManifestPart>,
    chapters: Vec<omnibus_shared::ChapterInfo>,
    pos_lit: &str,
    mut chapters_sig: Signal<Vec<omnibus_shared::ChapterInfo>>,
    mut hls_ready: Signal<bool>,
) {
    // Populate chapter signal from manifest data.
    chapters_sig.set(chapters);

    // Hand the part list to JS; initDirect picks
    // the right starting part by cumulative offset.
    let parts_json = serde_json::to_string(&parts).unwrap_or_else(|_| "[]".into());
    let init_js = format!(
        r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initDirect({parts_json}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else {{ console.error('OmnibusAudio never installed; init timed out'); if (typeof window.__omnibusOnInitTimeout === 'function') {{ window.__omnibusOnInitTimeout(0); }} }} }})(); }})();"#
    );
    let _ = dioxus::document::eval(&init_js);
    hls_ready.set(true);
}

/// HLS arm: poll `/api/audiobooks/{uuid}/status` until `ready` (call
/// `initHls` + flip `hls_ready`) or `failed` (flip `playback_failed`).
/// Re-checks `is_current` every iteration so a stale poll can't clobber
/// the newly-active book's signals.
async fn init_hls(
    uuid: &str,
    playlist_url: &str,
    pos_lit: &str,
    is_current: impl Fn() -> bool,
    mut hls_ready: Signal<bool>,
    mut playback_failed: Signal<bool>,
) {
    let playlist_lit = serde_json::to_string(playlist_url).unwrap_or_else(|_| "\"\"".into());
    loop {
        // Stop polling the moment the user switches away from this book,
        // so a stale `/status` loop can't flip the new book's signals.
        if !is_current() {
            return;
        }
        match fetch_hls_status(uuid).await.as_deref() {
            // Bug 4 from #338: surface failed transcodes instead of
            // polling forever.
            Some("failed") => {
                playback_failed.set(true);
                return;
            }
            Some("ready") => {
                eval_hls_init(&playlist_lit, pos_lit);
                hls_ready.set(true);
                return;
            }
            _ => {}
        }
        gloo_timers::future::TimeoutFuture::new(1_000).await;
    }
}

/// One HLS `/status` poll: fetch + decode the JSON body and return the
/// `state` field, or `None` on network / decode failure (the caller
/// just keeps polling).
async fn fetch_hls_status(uuid: &str) -> Option<String> {
    let resp = gloo_net::http::Request::get(&format!("/api/audiobooks/{uuid}/status"))
        .send()
        .await
        .ok()?;
    if resp.status() != 200 {
        return None;
    }
    let json: serde_json::Value = resp.json().await.ok()?;
    json.get("state")
        .and_then(|v| v.as_str())
        .map(str::to_owned)
}

/// Once the HLS transcode reports `ready`, eval the JS that polls for
/// `window.OmnibusAudio` and calls `initHls`. Same mount-race shield as
/// the Direct arm — `__omnibusOnInitTimeout` surfaces a UI failure if
/// the shim never installs.
fn eval_hls_init(playlist_lit: &str, pos_lit: &str) {
    let init_js = format!(
        r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initHls({playlist_lit}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else {{ console.error('OmnibusAudio never installed; HLS init timed out'); if (typeof window.__omnibusOnInitTimeout === 'function') {{ window.__omnibusOnInitTimeout(0); }} }} }})(); }})();"#
    );
    let _ = dioxus::document::eval(&init_js);
}

// Tests for the pure Rust-side logic only. The JS interop seams
// (register_js_callbacks, inject_hls_script, install_control_surface,
// run_manifest_init) require a WASM runtime and are covered by Playwright
// at ui_tests/playwright/tests/flows/.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_resume_pos_prefers_server_position_when_present() {
        assert!((resolve_resume_pos(Some(120.0), 5.0) - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_resume_pos_falls_back_to_local_when_server_absent() {
        assert!((resolve_resume_pos(None, 42.5) - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_resume_pos_returns_zero_local_when_both_absent() {
        assert!((resolve_resume_pos(None, 0.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn control_surface_js_contains_expected_segments() {
        let js = control_surface_js("1.25", "\"abc-123\"");
        // Rust-side interpolation points landed.
        assert!(js.contains("el.playbackRate = 1.25;"));
        assert!(js.contains("_uuid: \"abc-123\","));
        // Each pure JS segment contributed its signature substring.
        assert!(
            js.contains("SPA-nav from another page"),
            "dom_reset_js missing"
        );
        assert!(js.contains("function absTime()"), "listeners_js missing");
        assert!(
            js.contains("initDirect: function(parts"),
            "direct_play_init_js missing"
        );
        assert!(js.contains("initHls: function(url"), "hls_init_js missing");
        // Sanity: no stray `{{` / `}}` escape pairs leaked from a `format!`.
        assert!(!js.contains("{{"), "literal {{ leaked into JS");
        assert!(!js.contains("}}"), "literal }} leaked into JS");
    }
}
