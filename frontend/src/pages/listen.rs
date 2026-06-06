//! F2.3 immersive audiobook player with direct-play + HLS fallback (#339).
//!
//! Renders a full-screen "Now playing" surface: cover + title + author on the
//! left, scrub bar + transport controls on the right.
//!
//! Startup sequence:
//! 1. `GET /api/audiobooks/{uuid}/manifest` returns either
//!    `{mode: "direct", parts}` (m4b/m4a/mp3/aac — instant) or
//!    `{mode: "hls", playlist_url}` (everything else).
//! 2. For direct mode, the JS shim chains per-part `<audio src>` URLs with
//!    auto-advance on `ended`; the book's timeline is a cumulative sum of
//!    per-part durations.
//! 3. For HLS mode, fall back to the legacy `/status` poll + hls.js attach.
//!    If `/status` reports `state: "failed"`, render an error overlay
//!    instead of polling forever (Bug 4 from #338).
//!
//! Position lives in [`crate::audiobook_progress`] — localStorage on web,
//! in-memory on mobile, no-op under SSR. Writes both there AND fire-and-
//! forget POST `/api/rpc/progress` (F2.1) with `format: "audio"` +
//! `audio_position_seconds`, so a position written on one device syncs
//! forward on the next open. Across direct-mode parts the JS shim reports
//! absolute (cross-part) seconds so the same shape works for both modes.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::{use_navigator, Link};
#[cfg(not(feature = "mobile"))]
use omnibus_shared::EbookMetadata;

#[cfg(not(feature = "mobile"))]
use crate::components::atrium::Cover;
#[cfg(not(feature = "mobile"))]
use crate::{data, use_server_url, Route};

/// Available playback-rate steps. Clicking the rate button cycles forward.
#[cfg(not(feature = "mobile"))]
const RATE_STEPS: &[f64] = &[0.8, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Vendored hls.js for the HLS fallback path. Routed through `manganis::asset!`
/// so `dx serve` exposes it under the hashed `/assets/...` URL it actually
/// serves — a hard-coded `/assets/vendor/hls.min.js` 404s.
#[cfg(feature = "web")]
const HLS_JS: Asset = asset!("/assets/vendor/hls.min.js");

/// Single audited surface for poking `window.OmnibusAudio`. Same shape as
/// `reader.rs::reader_call` — `method` is always a hard-coded identifier and
/// `arg_js` is empty or a `serde_json`-encoded literal.
#[cfg(feature = "web")]
fn audio_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusAudio && window.OmnibusAudio.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

#[component]
pub fn BookListenPage(uuid: String) -> Element {
    #[cfg(feature = "mobile")]
    {
        // Mobile native shell has no `<audio>` element binding yet — same
        // gate as the EPUB reader. Stub a placeholder so the route still
        // compiles for the mobile target.
        let _ = uuid;
        return rsx! {
            div { class: "screen",
                p { class: "subtitle", "Audiobook playback on mobile is coming soon." }
            }
        };
    }

    #[cfg(not(feature = "mobile"))]
    {
        let server_url = use_server_url();
        let mut book: Signal<Option<EbookMetadata>> = use_signal(|| None);
        let mut loading = use_signal(|| true);
        let mut error: Signal<Option<String>> = use_signal(|| None);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut duration = use_signal(|| 0.0_f64);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut elapsed = use_signal(|| 0.0_f64);
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut playing = use_signal(|| false);
        let mut rate = use_signal(crate::audiobook_progress::load_rate);
        // Playback readiness: false until initDirect / initHls has fired.
        // For direct mode this flips on as soon as the manifest fetch
        // returns; for HLS it waits for `/status` to report `ready`.
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut hls_ready = use_signal(|| false);
        // Terminal playback failure: HLS transcode `.failed` marker
        // present, or manifest fetch failed. Bug 4 from #338 — distinct
        // from "preparing" so the UI can render an error rather than
        // spinning forever.
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut playback_failed = use_signal(|| false);

        let url = server_url.clone();
        let uuid_for_fetch = uuid.clone();
        use_effect(use_reactive!(|uuid_for_fetch| {
            let url = url.clone();
            let uuid = uuid_for_fetch.clone();
            spawn(async move {
                loading.set(true);
                match data::get_ebook(&url, &uuid).await {
                    Ok(b) => {
                        book.set(b);
                        error.set(None);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                loading.set(false);
            });
        }));

        // ── Web interop: HLS status poll + audio element binding ──────────
        #[cfg(feature = "web")]
        {
            use wasm_bindgen::prelude::*;

            let cb_holder: std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(f64)>>>> =
                use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));

            let uuid_for_mount = uuid.clone();
            let uuid_for_cb = uuid.clone();
            use_effect(use_reactive!(|uuid_for_mount| {
                let uuid = uuid_for_mount.clone();
                let uuid_cb = uuid_for_cb.clone();
                let initial_position = crate::audiobook_progress::load(&uuid).unwrap_or(0.0);
                let initial_rate = crate::audiobook_progress::load_rate();

                // Register Rust-side callbacks before the JS bootstrap so a
                // fast `loadedmetadata` always finds them.
                if let Some(window) = web_sys::window() {
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
                    let uuid_for_pause = uuid_cb.clone();
                    let on_pause = Closure::<dyn FnMut(f64)>::new(move |secs: f64| {
                        playing.set(false);
                        crate::audiobook_progress::save(&uuid_for_pause, secs);
                        post_audio_progress(uuid_for_pause.clone(), secs);
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
                    *cb_holder.borrow_mut() = vec![on_time, on_duration, on_play, on_pause];
                }

                // Inject hls.js (one-time; the script tag is idempotent because
                // the browser caches it by URL). Resolved through manganis so
                // the URL matches what `dx serve` actually serves.
                let hls_src =
                    serde_json::to_string(&HLS_JS.to_string()).unwrap_or_else(|_| "\"\"".into());
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

                // Install the OmnibusAudio control surface immediately so
                // the transport buttons are wired even before initDirect /
                // initHls attaches. The two init paths are responsible for
                // setting their own initial position (per-part for direct,
                // absolute for hls) via one-shot loadedmetadata listeners.
                let rate_lit = serde_json::to_string(&initial_rate).unwrap_or_else(|_| "1".into());
                let uuid_lit = serde_json::to_string(&uuid).unwrap_or_else(|_| "\"\"".into());

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

                // Bootstrap: fetch the manifest once and branch on `mode`.
                // Direct mode (m4b / m4a / mp3 / aac) → call initDirect,
                // ready immediately. HLS mode (flac / ac3 / …) → fall
                // back to the legacy /status poll until ready or failed.
                let uuid_for_fetch = uuid.clone();
                spawn(async move {
                    // Reconcile resume position with the server upfront so
                    // both init paths see the same starting point.
                    let server_pos = data::get_progress(
                        "",
                        &uuid_for_fetch,
                        omnibus_shared::ProgressFormat::Audio,
                    )
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
                            let parts_json =
                                serde_json::to_string(&parts).unwrap_or_else(|_| "[]".into());
                            let init_js = format!(
                                r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initDirect({parts_json}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} }})(); }})();"#
                            );
                            let _ = dioxus::document::eval(&init_js);
                            hls_ready.set(true);
                        }
                        Some(omnibus_shared::AudiobookManifest::Hls { playlist_url }) => {
                            let playlist_lit = serde_json::to_string(&playlist_url)
                                .unwrap_or_else(|_| "\"\"".into());
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
                                                    r#"(function(){{ var n=0; (function go(){{ if (window.OmnibusAudio) {{ window.OmnibusAudio.initHls({playlist_lit}, {pos_lit}); }} else if (n++ < 200) {{ setTimeout(go, 50); }} }})(); }})();"#
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
                });
            }));
        }

        if loading() {
            return rsx! { p { class: "subtitle", "Loading\u{2026}" } };
        }
        if let Some(msg) = error() {
            return rsx! {
                p { role: "alert", class: "subtitle", "{msg}" }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            };
        }
        let Some(b) = book() else {
            return rsx! {
                p { class: "subtitle", "Audiobook not found." }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            };
        };

        let nav = use_navigator();
        let on_back = move |_| {
            nav.go_back();
        };

        let on_toggle = move |_| {
            #[cfg(feature = "web")]
            audio_call("toggle", "");
        };
        let on_skip_back = move |_| {
            #[cfg(feature = "web")]
            audio_call("skip", "-30");
        };
        let on_skip_forward = move |_| {
            #[cfg(feature = "web")]
            audio_call("skip", "30");
        };
        let on_seek = move |evt: Event<FormData>| {
            if let Ok(_secs) = evt.value().parse::<f64>() {
                #[cfg(feature = "web")]
                audio_call("seek", &_secs.to_string());
            }
        };
        let on_rate = move |_| {
            let cur = rate();
            let next = RATE_STEPS
                .iter()
                .copied()
                .find(|r| *r > cur + f64::EPSILON)
                .unwrap_or(RATE_STEPS[0]);
            rate.set(next);
            crate::audiobook_progress::save_rate(next);
            #[cfg(feature = "web")]
            audio_call("setRate", &next.to_string());
        };

        let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
        let author = b
            .creators
            .first()
            .map(|c| c.name.clone())
            .unwrap_or_else(|| "Unknown Author".to_string());
        let dur = duration();
        let elapsed_now = elapsed();
        let remaining = (dur - elapsed_now).max(0.0);
        let scrub_max = if dur > 0.0 { dur } else { 1.0 };
        let rate_label = format!("{:.2}\u{00d7}", rate());
        let play_label = if playing() { "Pause" } else { "Play" };
        let ready = hls_ready();
        let failed = playback_failed();

        rsx! {
            div {
                class: "player-root",
                style: "display:flex; flex-direction:column; height:100vh; width:100%; background:var(--bg-0);",

                // Slim top control bar.
                div {
                    class: "player-bar",
                    style: "display:flex; align-items:center; gap:0.5rem; padding:0.5rem 0.75rem; border-bottom:1px solid var(--line);",
                    button {
                        class: "btn ghost sm",
                        r#type: "button",
                        "data-testid": "listen-back",
                        "aria-label": "Back",
                        onclick: on_back,
                        "\u{2190} Back"
                    }
                    div { style: "flex:1;" }
                    span {
                        class: "label",
                        style: "color:var(--ink-2); font-family:var(--mono); font-size:12px;",
                        "Now playing"
                    }
                }

                // The audio element is invisible — transport chrome is below.
                audio {
                    id: "omnibus-audio",
                    "data-testid": "listen-audio",
                    style: "display:none;",
                    preload: "auto",
                }

                // Terminal failure (HLS .failed marker or manifest fetch
                // failed). Distinct from "preparing" — manual reload is
                // the recovery path. Bug 4 from #338.
                if failed {
                    div {
                        "data-testid": "listen-failed",
                        role: "alert",
                        style: "position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; background:var(--bg-0); z-index:10; gap:0.5rem;",
                        p {
                            style: "font-family:var(--serif); font-size:1.2rem; color:var(--ink-1);",
                            "Playback failed."
                        }
                        p {
                            style: "font-family:var(--mono); font-size:0.85rem; color:var(--ink-3); max-width:32rem; text-align:center;",
                            "The audiobook could not be prepared. Reload the page to try again, or check the server logs for a transcode failure."
                        }
                    }
                } else if !ready {
                    // Shown only while we wait for the HLS transcode —
                    // direct-play books flip `ready` true as soon as the
                    // manifest fetch returns, so this overlay never
                    // renders for them.
                    div {
                        "data-testid": "listen-preparing",
                        style: "position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; background:var(--bg-0); z-index:10;",
                        p {
                            style: "font-family:var(--serif); font-size:1.2rem; color:var(--ink-1);",
                            "Preparing your audiobook\u{2026}"
                        }
                        p {
                            style: "margin-top:0.5rem; font-family:var(--mono); font-size:0.85rem; color:var(--ink-3);",
                            "This may take a moment on first listen."
                        }
                    }
                }

                // Stage: cover on the left, "now playing" panel on the right.
                div {
                    class: "player-stage",
                    style: "flex:1; min-height:0; display:grid; grid-template-columns: 1fr 1fr; align-items:center; gap:48px; padding:0 48px;",
                    div {
                        style: "display:grid; place-items:center;",
                        div { style: "width:min(380px, 80%);",
                            Cover { book: b.clone() }
                        }
                    }
                    div {
                        h1 {
                            class: "player-title",
                            style: "font-family:var(--serif); font-style:italic; font-size:clamp(36px, 6vw, 72px); line-height:0.95; margin:0;",
                            "{title}"
                        }
                        div { style: "margin-top:12px; color:var(--ink-1); font-size:16px;",
                            "by {author}"
                        }

                        // Scrub bar + timestamps.
                        div { style: "margin-top:32px;",
                            input {
                                r#type: "range",
                                min: "0",
                                max: "{scrub_max}",
                                step: "0.5",
                                value: "{elapsed_now}",
                                "aria-label": "Seek",
                                "data-testid": "listen-scrub",
                                style: "width:100%;",
                                oninput: on_seek,
                            }
                            div {
                                style: "display:flex; justify-content:space-between; margin-top:8px; font-family:var(--mono); font-size:12px; color:var(--ink-2);",
                                span { "{format_hms(elapsed_now)}" }
                                span {
                                    style: "color:var(--ink-3);",
                                    "\u{00b7} {format_hms(remaining)} remaining"
                                }
                                span { "{format_hms(dur)}" }
                            }
                        }

                        // Transport controls.
                        div {
                            style: "display:flex; align-items:center; justify-content:center; gap:18px; margin-top:24px;",
                            button {
                                class: "btn ghost",
                                style: "width:48px; height:48px; padding:0; border-radius:999px; font-family:var(--mono); font-size:11px;",
                                r#type: "button",
                                "data-testid": "listen-skip-back",
                                "aria-label": "Back 30 seconds",
                                onclick: on_skip_back,
                                "-30"
                            }
                            button {
                                style: "width:72px; height:72px; border-radius:999px; background:var(--accent); color:var(--accent-ink); border:0; cursor:pointer; font-size:16px; font-weight:600;",
                                r#type: "button",
                                "data-testid": "listen-toggle",
                                "aria-label": play_label,
                                onclick: on_toggle,
                                "{play_label}"
                            }
                            button {
                                class: "btn ghost",
                                style: "width:48px; height:48px; padding:0; border-radius:999px; font-family:var(--mono); font-size:11px;",
                                r#type: "button",
                                "data-testid": "listen-skip-forward",
                                "aria-label": "Forward 30 seconds",
                                onclick: on_skip_forward,
                                "+30"
                            }
                            button {
                                class: "btn ghost",
                                style: "min-width:48px; height:40px; padding:0 12px; border-radius:999px; font-family:var(--mono); font-size:12px;",
                                r#type: "button",
                                "data-testid": "listen-rate",
                                "aria-label": "Playback speed",
                                onclick: on_rate,
                                "{rate_label}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Format `seconds` as `H:MM:SS` (or `MM:SS` when under an hour).
/// Negative / non-finite values clamp to `0:00`.
#[cfg_attr(feature = "mobile", allow(dead_code))]
fn format_hms(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let s_total = seconds as u64;
    let h = s_total / 3600;
    let m = (s_total % 3600) / 60;
    let s = s_total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Fire-and-forget POST `/api/rpc/progress` with the audio update.
/// Mirrors the EPUB reader's payload pattern so both formats produce the same
/// `ProgressUpdate` shape (`format: "audio"` + `audio_position_seconds`).
/// Errors are intentionally ignored — the local cache is the safety net.
#[cfg(feature = "web")]
fn post_audio_progress(uuid: String, seconds: f64) {
    wasm_bindgen_futures::spawn_local(async move {
        let body = serde_json::json!({
            "update": {
                "book_uuid": uuid,
                "format": "audio",
                "audio_position_seconds": seconds,
            }
        });
        if let Ok(req) = gloo_net::http::Request::post("/api/rpc/progress").json(&body) {
            let _ = req.send().await;
        }
    });
}

#[cfg(not(feature = "web"))]
#[allow(dead_code)]
fn post_audio_progress(_uuid: String, _seconds: f64) {}

#[cfg(test)]
mod tests {
    use super::format_hms;

    #[test]
    fn format_hms_under_one_hour_renders_mm_ss() {
        assert_eq!(format_hms(0.0), "0:00");
        assert_eq!(format_hms(5.0), "0:05");
        assert_eq!(format_hms(65.0), "1:05");
        assert_eq!(format_hms(599.9), "9:59");
    }

    #[test]
    fn format_hms_past_one_hour_renders_h_mm_ss() {
        assert_eq!(format_hms(3600.0), "1:00:00");
        assert_eq!(format_hms(3661.0), "1:01:01");
        assert_eq!(format_hms(13_596.0), "3:46:36");
    }

    #[test]
    fn format_hms_handles_negative_and_non_finite_as_zero() {
        assert_eq!(format_hms(-12.0), "0:00");
        assert_eq!(format_hms(f64::NAN), "0:00");
        assert_eq!(format_hms(f64::INFINITY), "0:00");
    }
}
