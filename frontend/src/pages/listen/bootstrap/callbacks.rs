//! The JS→Rust callback surface: the closures `window.__omnibusOn*` calls
//! back into (position, transport, readiness) and their registration on
//! `window`. [`super`] owns their lifetime via the callback holder.

use dioxus::prelude::*;
use wasm_bindgen::prelude::*;

use super::super::helpers::post_audio_progress;
use super::{JsCallbackHolder, NEXT_SEQUENCE_FILE};

/// Wire the six Rust-side closures that JS calls back into:
/// `__omnibusOnAudioTime`, `__omnibusOnAudioDuration`, `__omnibusOnAudioPlay`,
/// `__omnibusOnAudioPause`, `__omnibusOnAudioEnded`, `__omnibusOnInitTimeout`.
/// Registered before the JS bootstrap so a fast `loadedmetadata` always finds
/// them.
///
/// `loaded_file_id` is the shared signal `run_manifest_init` fills once the
/// manifest names the file it resolved; every position POST reads it live,
/// so writes name the file playback is actually in even though these
/// closures are registered before the manifest lands (#1888). A file switch
/// always re-boots (see [`super::needs_reload`]), which resets the signal
/// before the next manifest fills it again.
#[expect(clippy::too_many_arguments, reason = "one closure per JS callback")]
pub(super) fn register_js_callbacks(
    cb_holder: &JsCallbackHolder,
    uuid_cb: String,
    loaded_file_id: Signal<Option<i64>>,
    mut duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    mut playback_failed: Signal<bool>,
    file_sig: Signal<Option<i64>>,
    uuid_sig: Signal<Option<String>>,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let (on_time, on_seeked) = position_callbacks(uuid_cb.clone(), loaded_file_id, elapsed);
    let on_duration = Closure::<dyn FnMut(f64)>::new(move |d: f64| {
        duration.set(d);
    });
    let on_play = play_callback(uuid_cb.clone(), playing);
    let on_ended = ended_callback(uuid_cb.clone(), file_sig, uuid_sig);
    let on_pause = pause_callback(uuid_cb, loaded_file_id, playing);
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
        &JsValue::from_str("__omnibusOnAudioSeeked"),
        on_seeked.as_ref().unchecked_ref(),
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
        &JsValue::from_str("__omnibusOnAudioEnded"),
        on_ended.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnInitTimeout"),
        on_init_timeout.as_ref().unchecked_ref(),
    );
    *cb_holder.borrow_mut() = vec![
        Box::new(on_time),
        Box::new(on_seeked),
        Box::new(on_duration),
        Box::new(on_play),
        Box::new(on_pause),
        Box::new(on_ended),
        Box::new(on_init_timeout),
    ];
}

/// The throttled heartbeat plus the immediate seek flush.
type PositionCallbacks = (Closure<dyn FnMut(f64, bool)>, Closure<dyn FnMut(f64)>);

/// The two position-writing closures — the throttled heartbeat and the
/// immediate seek flush. They share a `last_saved` cell so a seek resets
/// the heartbeat's 5s window rather than double-writing moments later.
fn position_callbacks(
    uuid_cb: String,
    loaded_file_id: Signal<Option<i64>>,
    mut elapsed: Signal<f64>,
) -> PositionCallbacks {
    let uuid_for_save = uuid_cb;
    let last_saved = std::rc::Rc::new(std::cell::Cell::new(0.0_f64));
    let on_time = {
        let last_saved = last_saved.clone();
        let uuid_for_save = uuid_for_save.clone();
        // `user_acted` is the shim's `_userActed`: false until a real play
        // or user seek this boot. The boot restore sets `currentTime`,
        // which fires a `timeupdate` — persisting that write re-clocked an
        // unmoved row on every visit and shadowed the counterpart at the
        // cross-format clock gate (#1972). Render the time; persist only
        // positions a human produced.
        Closure::<dyn FnMut(f64, bool)>::new(move |secs: f64, user_acted: bool| {
            elapsed.set(secs);
            if !user_acted {
                return;
            }
            if (secs - last_saved.get()).abs() < 5.0 {
                return;
            }
            last_saved.set(secs);
            crate::audiobook_progress::save(&uuid_for_save, secs);
            post_audio_progress(uuid_for_save.clone(), *loaded_file_id.peek(), secs);
        })
    };
    // A seek is discrete user intent — persist it immediately, bypassing
    // the heartbeat throttle. A paused element fires no further events, so
    // waiting on the next timeupdate loses the seek if the page exits
    // first (#1972).
    let on_seeked = {
        let uuid_for_seek = uuid_for_save;
        Closure::<dyn FnMut(f64)>::new(move |secs: f64| {
            elapsed.set(secs);
            last_saved.set(secs);
            crate::audiobook_progress::save(&uuid_for_seek, secs);
            post_audio_progress(uuid_for_seek.clone(), *loaded_file_id.peek(), secs);
        })
    };
    (on_time, on_seeked)
}

/// Starting an `Unread` book marks it `Reading`, the audio counterpart of
/// the readers' mark-on-open. Fired from the first play of this boot only
/// — `play` also fires on every resume from pause, and a file switch
/// re-registers these callbacks, so the flag scopes it to one write per
/// book opened.
fn play_callback(uuid_for_start: String, mut playing: Signal<bool>) -> Closure<dyn FnMut(f64)> {
    let mut marked_reading = false;
    Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        playing.set(true);
        if marked_reading {
            return;
        }
        marked_reading = true;
        let uuid = uuid_for_start.clone();
        wasm_bindgen_futures::spawn_local(async move {
            crate::read_status_auto::apply_auto_read_status("", &uuid, false).await;
        });
    })
}

/// This FILE played through (the `ended` handler in `js.rs` only calls
/// once there is no next part). On a multi-file sequence the book is
/// not done — advance the player to the next file's start (paused;
/// the boot leaves it unseeded until a real play) instead of marking
/// five-hours-in as finished. Only the last file finishes the book.
fn ended_callback(
    uuid_for_end: String,
    file_sig: Signal<Option<i64>>,
    uuid_sig: Signal<Option<String>>,
) -> Closure<dyn FnMut(f64)> {
    Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        if let Some(next) = NEXT_SEQUENCE_FILE.with(|c| c.get()) {
            let uuid = uuid_for_end.clone();
            let mut file_sig = file_sig;
            let mut uuid_sig = uuid_sig;
            file_sig.set(Some(next));
            // The driver keys on the uuid changing; a None→Some pair in
            // one render collapses to no-change, so the Some lands from a
            // spawned task after the None commits.
            uuid_sig.set(None);
            wasm_bindgen_futures::spawn_local(async move {
                uuid_sig.set(Some(uuid));
            });
            return;
        }
        let uuid = uuid_for_end.clone();
        wasm_bindgen_futures::spawn_local(async move {
            crate::read_status_auto::apply_auto_read_status("", &uuid, true).await;
        });
    })
}

/// The playing flag flips on every pause (src swaps and teardown fire
/// them too), but only a session the user actually drove persists its
/// position — a programmatic pause on an untouched boot re-states the
/// seeded spot with a fresh clock (#1972).
fn pause_callback(
    uuid_for_pause: String,
    loaded_file_id: Signal<Option<i64>>,
    mut playing: Signal<bool>,
) -> Closure<dyn FnMut(f64, bool)> {
    Closure::<dyn FnMut(f64, bool)>::new(move |secs: f64, user_acted: bool| {
        playing.set(false);
        if !user_acted {
            return;
        }
        crate::audiobook_progress::save(&uuid_for_pause, secs);
        post_audio_progress(uuid_for_pause.clone(), *loaded_file_id.peek(), secs);
    })
}

/// Wire the three JS→Rust readiness/health callbacks:
/// `__omnibusOnAudioBooted` fires once `initDirect`/`initHls` has actually
/// committed a source to the element — the authoritative "ready" signal,
/// replacing the old optimistic `hls_ready.set(true)` issued right after
/// firing the init eval (a real, if usually tiny, race against the JS
/// actually running). `__omnibusOnAudioBuffering` mirrors the JS
/// watchdog's `readyState < HAVE_FUTURE_DATA` check while playing.
/// `__omnibusOnAudioStalled` reuses `playback_failed` — a stall with no
/// forward progress is exactly as unrecoverable-without-a-reload as an
/// init timeout. Closures are appended to `cb_holder` alongside
/// [`register_js_callbacks`]'s so both sets outlive the installing effect.
pub(super) fn register_readiness_callbacks(
    cb_holder: &JsCallbackHolder,
    mut hls_ready: Signal<bool>,
    mut buffering: Signal<bool>,
    mut playback_failed: Signal<bool>,
) {
    let Some(window) = web_sys::window() else {
        return;
    };
    let on_booted = Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        hls_ready.set(true);
    });
    let on_buffering = Closure::<dyn FnMut(f64)>::new(move |flag: f64| {
        buffering.set(flag > 0.5);
    });
    let on_stalled = Closure::<dyn FnMut(f64)>::new(move |_: f64| {
        playback_failed.set(true);
    });
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioBooted"),
        on_booted.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioBuffering"),
        on_buffering.as_ref().unchecked_ref(),
    );
    let _ = js_sys::Reflect::set(
        &window,
        &JsValue::from_str("__omnibusOnAudioStalled"),
        on_stalled.as_ref().unchecked_ref(),
    );
    cb_holder.borrow_mut().extend([
        Box::new(on_booted) as Box<dyn std::any::Any>,
        Box::new(on_buffering),
        Box::new(on_stalled),
    ]);
}
