//! Web-only `OmnibusAudio` bootstrap: installs the JS control surface on
//! `window`, wires Rust callbacks for time / duration / play / pause, then
//! fetches the manifest and branches on `mode` (direct vs hls) to seed
//! playback. Extracted from `BookListenPage` so the parent stays under the
//! 150-line component cap; everything here is web-feature gated.

#![cfg(feature = "web")]

use dioxus::prelude::*;
use wasm_bindgen::prelude::*;

use crate::data;

use super::helpers::{post_audio_progress, HLS_JS};

/// Install the `window.OmnibusAudio` shim and kick off the manifest-driven
/// init effect. Returns nothing — all state lives in the passed-in signals.
///
/// `book_uuid` must be the live `uuid` from the page props so the
/// `use_reactive!` retriggers when the route param changes (SPA-nav from
/// one audiobook to the next).
pub(super) fn install_audio_bootstrap(
    book_uuid: String,
    duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    hls_ready: Signal<bool>,
    playback_failed: Signal<bool>,
) {
    let cb_holder: std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(f64)>>>> =
        use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

    let uuid_for_mount = book_uuid.clone();
    let uuid_for_cb = book_uuid;
    use_effect(use_reactive!(|uuid_for_mount| {
        let uuid = uuid_for_mount.clone();
        let uuid_cb = uuid_for_cb.clone();
        let initial_position = crate::audiobook_progress::load(&uuid).unwrap_or(0.0);
        let initial_rate = crate::audiobook_progress::load_rate();

        register_js_callbacks(
            &cb_holder,
            uuid_cb.clone(),
            duration,
            elapsed,
            playing,
            playback_failed,
        );
        inject_hls_script();
        install_control_surface(&uuid, initial_rate);

        // Bootstrap: fetch the manifest once and branch on `mode`.
        // Direct mode (m4b / m4a / mp3 / aac) → call initDirect,
        // ready immediately. HLS mode (flac / ac3 / …) → fall
        // back to the legacy /status poll until ready or failed.
        let uuid_for_fetch = uuid.clone();
        spawn(async move {
            run_manifest_init(uuid_for_fetch, initial_position, hls_ready, playback_failed).await;
        });
    }));
}

/// Wire the five Rust-side closures that JS calls back into:
/// `__omnibusOnAudioTime`, `__omnibusOnAudioDuration`, `__omnibusOnAudioPlay`,
/// `__omnibusOnAudioPause`, `__omnibusOnInitTimeout`. Registered before the
/// JS bootstrap so a fast `loadedmetadata` always finds them.
fn register_js_callbacks(
    cb_holder: &std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(f64)>>>>,
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

    let js = format!(
        r#"
(function(){{
  // SPA-nav from another page leaves a stale `window.OmnibusAudio` from
  // the previous visit, captured in a closure over a now-detached
  // `<audio>` element. The init poll below sees that stale object,
  // calls `initDirect` on it, and the visible audio element never gets
  // a src — the scrub bar reads 0:00 until a full reload. Clearing
  // here forces the init poll to wait for the fresh install.
  try {{ var _prev = window.OmnibusAudio; if (_prev) {{ _prev._stale = true; }} }} catch(_) {{}}
  window.OmnibusAudio = null;
  // Wait for the audio element to appear in the DOM.
  var n = 0;
  function mount(){{
    var el = document.getElementById('omnibus-audio');
    if (!el) {{ if (n++ < 200) {{ return setTimeout(mount, 50); }} else {{ return; }} }}
    // Reset the element so leftover src / preloading from a prior mount
    // doesn't keep streaming once we swap modes.
    try {{ el.pause(); }} catch(_) {{}}
    el.removeAttribute('src');
    el.preload = 'auto';
    el.playbackRate = {rate_lit};

    // Helper: absolute seconds across the whole book. For direct mode
    // this adds the current part's cumulative offset; for HLS the audio
    // element's `currentTime` already IS absolute (one continuous
    // timeline).
    function absTime() {{
      var oa = window.OmnibusAudio;
      if (oa && oa._mode === 'direct' && oa._parts) {{
        return (oa._cumOffsets[oa._index] || 0) + (el.currentTime || 0);
      }}
      return el.currentTime || 0;
    }}

    el.addEventListener('loadedmetadata', function(){{
      // Initial seek is the job of init{{Direct,Hls}} via their own
      // one-shot loadedmetadata listeners — this listener only reports
      // duration. For direct mode we always report the book-level total
      // so a part change does not collapse the scrub bar to per-part.
      var oa = window.OmnibusAudio;
      if (window.__omnibusOnAudioDuration) {{
        var d = (oa && oa._mode === 'direct' && oa._totalDuration > 0)
          ? oa._totalDuration
          : (el.duration || 0);
        window.__omnibusOnAudioDuration(d);
      }}
    }});
    el.addEventListener('timeupdate', function(){{
      if (window.__omnibusOnAudioTime) {{
        window.__omnibusOnAudioTime(absTime());
      }}
    }});
    el.addEventListener('play', function(){{
      if (window.__omnibusOnAudioPlay) {{
        window.__omnibusOnAudioPlay(absTime());
      }}
    }});
    el.addEventListener('pause', function(){{
      if (window.__omnibusOnAudioPause) {{
        window.__omnibusOnAudioPause(absTime());
      }}
    }});
    // Cross-part advance — direct mode only. HLS treats the whole
    // book as one continuous stream so `ended` only fires at the
    // actual end (which we leave as-is so the UI naturally stops).
    el.addEventListener('ended', function(){{
      var oa = window.OmnibusAudio;
      if (!oa || oa._mode !== 'direct' || !oa._parts) return;
      if (oa._index + 1 < oa._parts.length) {{
        oa._index += 1;
        el.src = oa._parts[oa._index].url;
        el.load();
        var p = el.play(); if (p && p.catch) p.catch(function(){{}});
      }}
    }});

    window.OmnibusAudio = {{
      // Playback mode. Set by initDirect / initHls; null before either fires.
      _mode: null,
      // Direct-mode state — null in HLS mode.
      _parts: null,
      _index: 0,
      _cumOffsets: [],
      _totalDuration: 0,

      play:    function(){{ var p = el.play(); if (p && p.catch) p.catch(function(){{}}); }},
      pause:   function(){{ el.pause(); }},
      toggle:  function(){{ if (el.paused) {{ this.play(); }} else {{ this.pause(); }} }},
      setRate: function(r){{ try {{ el.playbackRate = r; }} catch(_) {{}} }},

      // Seek to an absolute (cross-part) second offset. For direct mode
      // this finds the target part by cumulative duration and switches
      // `el.src` if it differs from the current part, preserving the
      // play/pause state across the swap.
      seek: function(absSeconds){{
        var s = Math.max(0, absSeconds || 0);
        if (this._mode === 'direct' && this._parts) {{
          var i = 0;
          while (i < this._cumOffsets.length - 1 && s >= this._cumOffsets[i + 1]) i++;
          var local = s - this._cumOffsets[i];
          if (i !== this._index) {{
            var wasPlaying = !el.paused;
            this._index = i;
            var onMeta = function(){{
              el.removeEventListener('loadedmetadata', onMeta);
              try {{ el.currentTime = local; }} catch(_) {{}}
              if (wasPlaying) {{
                var p = el.play(); if (p && p.catch) p.catch(function(){{}});
              }}
            }};
            el.addEventListener('loadedmetadata', onMeta);
            el.src = this._parts[i].url;
            el.load();
          }} else {{
            try {{ el.currentTime = local; }} catch(_) {{}}
          }}
        }} else {{
          try {{ el.currentTime = s; }} catch(_) {{}}
        }}
      }},

      // Relative skip (+30 / -30). Computed in absolute terms so a skip
      // straddling a part boundary works correctly.
      skip: function(d){{ this.seek(absTime() + d); }},

      // Direct-play init: parts is the array from the manifest endpoint
      // (`[{{ordinal, url, duration_seconds, mime}}]`), initialPositionAbs
      // is the absolute resume position in seconds.
      initDirect: function(parts, initialPositionAbs){{
        this._mode = 'direct';
        this._parts = parts;
        var acc = 0;
        this._cumOffsets = [];
        for (var i = 0; i < parts.length; i++) {{
          this._cumOffsets.push(acc);
          acc += parts[i].duration_seconds || 0;
        }}
        this._totalDuration = acc;
        // Push the total duration up to Rust eagerly so the scrub bar
        // gets the right max before the first part's metadata loads.
        if (window.__omnibusOnAudioDuration) {{
          window.__omnibusOnAudioDuration(this._totalDuration);
        }}
        // Pick the starting part for the resume position.
        var s = Math.max(0, initialPositionAbs || 0);
        var idx = 0;
        while (idx < this._cumOffsets.length - 1 && s >= this._cumOffsets[idx + 1]) idx++;
        this._index = idx;
        var local = s - this._cumOffsets[idx];
        var onMeta = function(){{
          el.removeEventListener('loadedmetadata', onMeta);
          try {{ el.currentTime = local; }} catch(_) {{}}
        }};
        el.addEventListener('loadedmetadata', onMeta);
        el.src = parts[idx].url;
        el.load();
      }},

      // HLS init: legacy fallback path for codecs the browser does not
      // play natively. `initialPositionAbs` is optional — Rust passes
      // `null` to skip the seek.
      initHls: function(url, initialPositionAbs){{
        this._mode = 'hls';
        if (typeof Hls !== 'undefined' && Hls.isSupported()) {{
          var hls = new Hls();
          hls.loadSource(url);
          hls.attachMedia(el);
          hls.on(Hls.Events.ERROR, function(_, d) {{
            if (d.fatal && window.__omnibusOnAudioPause) {{
              window.__omnibusOnAudioPause(el.currentTime || 0);
            }}
          }});
        }} else if (el.canPlayType('application/vnd.apple.mpegurl')) {{
          // Safari / iOS native HLS.
          el.src = url;
          el.load();
        }} else {{
          console.warn('OmnibusAudio: no HLS support in this browser');
        }}
        if (typeof initialPositionAbs === 'number' && initialPositionAbs > 0) {{
          var onMeta = function(){{
            el.removeEventListener('loadedmetadata', onMeta);
            try {{ el.currentTime = Math.max(0, initialPositionAbs); }} catch(_) {{}}
          }};
          el.addEventListener('loadedmetadata', onMeta);
        }}
      }},

      _uuid: {uuid_lit},
    }};
  }}
  mount();
}})();
"#
    );
    let _ = dioxus::document::eval(&js);
}

/// Fetch `/api/audiobooks/{uuid}/manifest` and either:
/// * **Direct mode** — call `initDirect` with the parts list and flip
///   `hls_ready` true (instant playback for m4b/m4a/mp3/aac).
/// * **HLS mode** — poll `/status` until `ready` (call `initHls` + flip
///   `hls_ready`) or `failed` (flip `playback_failed`).
/// * **No manifest** — show the same failure overlay as a terminal HLS
///   transcode failure.
async fn run_manifest_init(
    uuid_for_fetch: String,
    initial_position: f64,
    mut hls_ready: Signal<bool>,
    mut playback_failed: Signal<bool>,
) {
    // Reconcile resume position with the server upfront so
    // both init paths see the same starting point.
    let server_pos = data::get_progress("", &uuid_for_fetch, omnibus_shared::ProgressFormat::Audio)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.audio_position_seconds);
    let resume_pos = server_pos.unwrap_or(initial_position);
    let pos_lit = serde_json::to_string(&resume_pos).unwrap_or_else(|_| "0".into());

    let manifest_url = format!("/api/audiobooks/{}/manifest", uuid_for_fetch);
    let manifest: Option<omnibus_shared::AudiobookManifest> =
        match gloo_net::http::Request::get(&manifest_url).send().await {
            Ok(resp) if resp.status() == 200 => resp.json().await.ok(),
            _ => None,
        };

    match manifest {
        Some(omnibus_shared::AudiobookManifest::Direct { parts, .. }) => {
            // Hand the part list to JS; initDirect picks
            // the right starting part by cumulative offset.
            // Poll for `window.OmnibusAudio` because the
            // mount script above sits behind a `setTimeout`
            // polling loop for the `<audio>` element — when
            // the manifest fetch resolves before the first
            // 50 ms tick fires (~15 ms RTT vs 50 ms),
            // OmnibusAudio is still undefined and a bare
            // `OmnibusAudio && …` short-circuits silently.
            // Mirrors the reader.rs pattern.
            let parts_json = serde_json::to_string(&parts).unwrap_or_else(|_| "[]".into());
            let init_js = format!(
                r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initDirect({parts_json}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else {{ console.error('OmnibusAudio never installed; init timed out'); if (typeof window.__omnibusOnInitTimeout === 'function') {{ window.__omnibusOnInitTimeout(0); }} }} }})(); }})();"#
            );
            let _ = dioxus::document::eval(&init_js);
            hls_ready.set(true);
        }
        Some(omnibus_shared::AudiobookManifest::Hls { playlist_url }) => {
            let playlist_lit =
                serde_json::to_string(&playlist_url).unwrap_or_else(|_| "\"\"".into());
            loop {
                match gloo_net::http::Request::get(&format!(
                    "/api/audiobooks/{}/status",
                    uuid_for_fetch
                ))
                .send()
                .await
                {
                    Ok(resp) if resp.status() == 200 => {
                        if let Ok(json) = resp.json::<serde_json::Value>().await {
                            let state = json
                                .get("state")
                                .and_then(|v| v.as_str())
                                .unwrap_or("preparing");
                            // Bug 4 from #338: surface
                            // failed transcodes instead of
                            // polling forever.
                            if state == "failed" {
                                playback_failed.set(true);
                                break;
                            }
                            if state == "ready" {
                                // Same mount-race as the
                                // Direct arm — poll for
                                // `window.OmnibusAudio`
                                // rather than relying on it
                                // being installed already.
                                let init_js = format!(
                                    r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initHls({playlist_lit}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} else {{ console.error('OmnibusAudio never installed; HLS init timed out'); if (typeof window.__omnibusOnInitTimeout === 'function') {{ window.__omnibusOnInitTimeout(0); }} }} }})(); }})();"#
                                );
                                let _ = dioxus::document::eval(&init_js);
                                hls_ready.set(true);
                                break;
                            }
                        }
                    }
                    _ => {}
                }
                gloo_timers::future::TimeoutFuture::new(1_000).await;
            }
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
