//! F2.3 immersive audiobook player with HLS streaming.
//!
//! Renders a full-screen "Now playing" surface: cover + title + author on the
//! left, scrub bar + transport controls on the right. Audio is streamed via
//! HLS from `/api/audiobooks/:uuid/playlist.m3u8` (built from stored
//! `book_file_parts` durations; segments served from the ffmpeg transcode
//! cache).
//!
//! Startup sequence:
//! 1. Poll `GET /api/audiobooks/{uuid}/status` every second until `ready`.
//! 2. Load hls.js and call `window.OmnibusAudio.initHls(playlistUrl)`.
//! 3. The HLS manifest drives segment fetches; the player timeline is
//!    continuous across all parts (a folder of mp3s appears as one book).
//!
//! Position lives in [`crate::audiobook_progress`] — localStorage on web,
//! in-memory on mobile, no-op under SSR. Writes both there AND fire-and-
//! forget POST `/api/rpc/progress` (F2.1) with `format: "audio"` +
//! `audio_position_seconds`, so a position written on one device syncs
//! forward on the next open.

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
        // HLS readiness: false until /status returns ready=true.
        #[cfg_attr(not(feature = "web"), allow(unused_mut))]
        let mut hls_ready = use_signal(|| false);

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
                // the browser caches it by URL).
                let _ = dioxus::document::eval(
                    r#"(function(){
                        if (window.Hls) return;
                        var s = document.createElement('script');
                        s.src = '/assets/vendor/hls.min.js';
                        s.async = true;
                        document.head.appendChild(s);
                    })();"#,
                );

                // Install the OmnibusAudio control surface immediately so
                // the transport buttons are wired even before HLS attaches.
                let pos_lit =
                    serde_json::to_string(&initial_position).unwrap_or_else(|_| "0".into());
                let rate_lit = serde_json::to_string(&initial_rate).unwrap_or_else(|_| "1".into());
                let uuid_lit = serde_json::to_string(&uuid).unwrap_or_else(|_| "\"\"".into());
                let playlist_url = format!("/api/audiobooks/{uuid}/playlist.m3u8");
                let playlist_lit =
                    serde_json::to_string(&playlist_url).unwrap_or_else(|_| "\"\"".into());

                let js = format!(
                    r#"
(function(){{
  // Wait for the audio element to appear in the DOM.
  var n = 0;
  function mount(){{
    var el = document.getElementById('omnibus-audio');
    if (!el) {{ if (n++ < 200) {{ return setTimeout(mount, 50); }} else {{ return; }} }}
    el.preload = 'auto';
    el.playbackRate = {rate_lit};
    el.addEventListener('loadedmetadata', function(){{
      try {{ el.currentTime = {pos_lit}; }} catch(_) {{}}
      if (window.__omnibusOnAudioDuration) {{
        window.__omnibusOnAudioDuration(el.duration || 0);
      }}
    }});
    el.addEventListener('timeupdate', function(){{
      if (window.__omnibusOnAudioTime) {{
        window.__omnibusOnAudioTime(el.currentTime || 0);
      }}
    }});
    el.addEventListener('play', function(){{
      if (window.__omnibusOnAudioPlay) {{
        window.__omnibusOnAudioPlay(el.currentTime || 0);
      }}
    }});
    el.addEventListener('pause', function(){{
      if (window.__omnibusOnAudioPause) {{
        window.__omnibusOnAudioPause(el.currentTime || 0);
      }}
    }});

    // HLS init helper — called by Rust once the /status endpoint says ready.
    window.OmnibusAudio = {{
      play:    function(){{ var p = el.play(); if (p && p.catch) p.catch(function(){{}}); }},
      pause:   function(){{ el.pause(); }},
      toggle:  function(){{ if (el.paused) {{ this.play(); }} else {{ this.pause(); }} }},
      seek:    function(s){{ try {{ el.currentTime = Math.max(0, s); }} catch(_) {{}} }},
      skip:    function(d){{ try {{ el.currentTime = Math.max(0, (el.currentTime||0) + d); }} catch(_) {{}} }},
      setRate: function(r){{ try {{ el.playbackRate = r; }} catch(_) {{}} }},
      initHls: function(url){{
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
          // No HLS support — show an error (handled by the Rust side).
          console.warn('OmnibusAudio: no HLS support in this browser');
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

                // Reconcile server-authoritative position.
                let uuid_for_fetch = uuid.clone();
                let playlist = playlist_lit.clone();
                spawn(async move {
                    // Poll /status until ready, then init HLS.
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
                                    let ready = json
                                        .get("ready")
                                        .and_then(|v| v.as_bool())
                                        .unwrap_or(false);
                                    if ready {
                                        hls_ready.set(true);
                                        // Init the HLS player on the JS side.
                                        let init_js = format!(
                                            "window.OmnibusAudio && window.OmnibusAudio.initHls({playlist});"
                                        );
                                        let _ = dioxus::document::eval(&init_js);
                                        // Server-authoritative seek position.
                                        let server_pos = data::get_progress(
                                            "",
                                            &uuid_for_fetch,
                                            omnibus_shared::ProgressFormat::Audio,
                                        )
                                        .await
                                        .ok()
                                        .flatten()
                                        .and_then(|r| r.audio_position_seconds);
                                        if let Some(pos) = server_pos {
                                            let pos_lit = serde_json::to_string(&pos)
                                                .unwrap_or_else(|_| "0".into());
                                            let seek_js = format!(
                                                "window.OmnibusAudio && window.OmnibusAudio.seek({pos_lit});"
                                            );
                                            let _ = dioxus::document::eval(&seek_js);
                                        }
                                        break;
                                    }
                                }
                            }
                            _ => {}
                        }
                        // 1-second poll interval.
                        gloo_timers::future::TimeoutFuture::new(1_000).await;
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

                // "Preparing your audiobook…" overlay shown while HLS transcodes.
                if !ready {
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
