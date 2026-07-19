//! Web-only `OmnibusAudio` bootstrap: installs the JS control surface on
//! `window`, wires Rust callbacks for time / duration / play / pause, then
//! fetches the manifest and branches on `mode` (direct vs hls) to seed
//! playback. Extracted from `BookListenPage` so the parent stays under the
//! 150-line component cap; everything here is web-feature gated.

#![cfg(feature = "web")]

use dioxus::prelude::*;
use wasm_bindgen::prelude::*;

use super::helpers::post_audio_progress;
use crate::data;

mod js;

use js::{control_surface_js, eval_hls_init, inject_hls_script};

/// Owns the `Closure`s wired into `window.__omnibusOn*` callbacks. Held
/// across re-renders by `use_hook` so the closures outlive the effect
/// closure that registers them.
type JsCallbackHolder = std::rc::Rc<std::cell::RefCell<Vec<Closure<dyn FnMut(f64)>>>>;

// Below: the pure Rust decision surface, extracted so it can be unit-tested.
// The rest of the module is JS/WASM interop (`eval`, `window.*` callbacks).

/// Select the resume position: prefer the server-authoritative value when
/// available, fall back to the locally cached initial position.
fn resolve_resume_pos(server_pos: Option<f64>, local_pos: f64) -> f64 {
    server_pos.unwrap_or(local_pos)
}

/// Whether the driver must (re)load for a newly-requested `(uuid, file_id)`.
///
/// A different book always reloads. The *same* book reloads only on an
/// explicit, different file selection — so picking another part in the file
/// picker switches parts, while the mini-dock (which relinks with no
/// `file_id`) resumes the current part instead of restarting from the first
/// file. Mirrors `pages::listen::mobile::host::needs_reload`.
fn needs_reload(loaded: &Option<(String, Option<i64>)>, uuid: &str, file_id: Option<i64>) -> bool {
    match loaded {
        Some((loaded_uuid, loaded_file)) if loaded_uuid == uuid => {
            file_id.is_some() && file_id != *loaded_file
        }
        _ => true,
    }
}

/// App-root playback driver. Installs the `window.OmnibusAudio` shim and
/// drives manifest-based playback off the app-wide [`crate::PlaybackState`].
///
/// Called once from [`crate::App`] (so the audio element and all signals
/// outlive any single route). The inner `use_effect` reacts to
/// `playback.uuid` and `playback.file_id`: retargeting either (the listen
/// page on mount / a picker selection, or a dock dismiss that clears the
/// uuid) loads, swaps, or tears down playback, gated by [`needs_reload`].
/// Because the signals now outlive the page, every book swap must reset the
/// prior book's state before installing fresh callbacks.
pub(crate) fn install_audio_bootstrap(playback: crate::PlaybackState) {
    let cb_holder: JsCallbackHolder =
        use_hook(|| std::rc::Rc::new(std::cell::RefCell::new(Vec::new())));
    let current_user = use_context::<crate::CurrentUser>().0;

    // Seed the session volume from the persisted preference once, post-mount
    // (not per book — `boot_new_book` never resets this signal). Declared
    // before the uuid-tracking effect below so it commits first and
    // `boot_new_book`'s `install_control_surface` call sees the real value
    // for even the very first book.
    use_effect(move || {
        let mut volume = playback.volume;
        volume.set(super::helpers::load_volume());
    });

    // Record listening time off the play/pause signals — the rows behind
    // the `/stats` aggregates. Web posts same-origin, so no server URL.
    let server_url = use_signal(String::new);
    crate::session_tracker::use_listening_session(playback.uuid, playback.playing, server_url);

    // What's currently booted: (book uuid, the file_id it was booted with).
    // Gates re-boot so a same-book file-pick reloads but a bare mini-dock
    // relink resumes the current part. Held across renders via `use_hook`.
    let mut loaded_key = use_hook(|| Signal::new(None::<(String, Option<i64>)>));

    use_effect(move || {
        let resolved_user = current_user();
        if matches!(resolved_user, Some(None)) {
            let mut uuid = playback.uuid;
            uuid.set(None);
            return;
        }
        // Reactive dependencies — re-run when the active book *or* the selected
        // file changes (the picker retargets `file_id` without changing uuid).
        let requested_uuid = playback.uuid.read().clone();
        let requested_file = *playback.file_id.read();
        let Some(uuid) = requested_uuid else {
            // Dismissed via the dock × button. The handler already stopped
            // the element; clear the book so the dock hides everywhere, and
            // forget the booted key so re-selecting the same book reloads.
            let mut book = playback.book;
            book.set(None);
            loaded_key.set(None);
            return;
        };
        if !needs_reload(&loaded_key.peek().clone(), &uuid, requested_file) {
            return;
        }
        loaded_key.set(Some((uuid.clone(), requested_file)));
        let user_id = resolved_user.flatten().map(|user| user.id);
        boot_new_book(
            &cb_holder,
            &uuid,
            requested_file,
            user_id,
            current_user,
            playback,
        );
    });
}

/// Reset the prior book's state and install fresh JS + Rust callbacks for
/// the newly-active `uuid`. Spawned tasks (metadata fetch + manifest init)
/// guard their writes on the live `playback.uuid` so a stale fetch can't
/// clobber a subsequent book swap.
fn boot_new_book(
    cb_holder: &JsCallbackHolder,
    uuid: &str,
    file_id: Option<i64>,
    user_id: Option<i64>,
    current_user: Signal<Option<Option<omnibus_shared::UserSummary>>>,
    playback: crate::PlaybackState,
) {
    // Signals outlive the page, so a swap must reset stale per-book state
    // before installing fresh callbacks — otherwise the previous book's
    // metadata/position/chapters leak under the new uuid until the async
    // fetches land. Clear the cross-book fields (book/error/chapters)
    // synchronously here, not just the playback scalars.
    reset_per_book_signals(&playback, user_id, uuid);

    let initial_position = crate::audiobook_progress::load(uuid).unwrap_or(0.0);
    let initial_rate = user_id
        .and_then(|id| crate::audiobook_progress::load_rate(id, uuid))
        .unwrap_or(1.0);
    // Session-wide, not per-book — re-read on every swap so a mid-session
    // volume change (or the sleep-timer fade's transient dip) doesn't leak
    // into the freshly-installed control surface.
    let initial_volume = *playback.volume.peek();

    register_js_callbacks(
        cb_holder,
        uuid.to_string(),
        playback.duration,
        playback.elapsed,
        playback.playing,
        playback.playback_failed,
    );
    inject_hls_script();
    install_control_surface(uuid, initial_rate, initial_volume);

    // Stale-task guard: the user can switch books while these async tasks
    // are in flight. Each task captures the uuid it was spawned for and
    // checks the live `playback.uuid` before writing any shared signal, so
    // a stale fetch / status-poll can't clobber the new book's state.
    let guard = playback.uuid;

    spawn_book_metadata_fetch(uuid.to_string(), guard, playback);
    spawn_manifest_init(
        uuid.to_string(),
        file_id,
        user_id,
        initial_position,
        playback,
        current_user,
    );
}

/// Synchronously clear playback scalars and cross-book fields before the
/// next book's async fetches land, so prior metadata/chapters don't leak.
fn reset_per_book_signals(playback: &crate::PlaybackState, user_id: Option<i64>, uuid: &str) {
    let mut duration = playback.duration;
    let mut elapsed = playback.elapsed;
    let mut playing = playback.playing;
    let mut hls_ready = playback.hls_ready;
    let mut playback_failed = playback.playback_failed;
    let mut rate = playback.rate;
    let mut rate_error = playback.rate_error;
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
    rate_error.set(None);
    rate.set(
        user_id
            .and_then(|id| crate::audiobook_progress::load_rate(id, uuid))
            .unwrap_or(1.0),
    );
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
/// active book's `file_id` (multi-file books) is the picker's selection,
/// threaded down from the driver's `(uuid, file_id)` reload key.
fn spawn_manifest_init(
    uuid: String,
    file_id: Option<i64>,
    user_id: Option<i64>,
    initial_position: f64,
    playback: crate::PlaybackState,
    current_user: Signal<Option<Option<omnibus_shared::UserSummary>>>,
) {
    spawn(async move {
        run_manifest_init(
            uuid,
            file_id,
            user_id,
            initial_position,
            playback,
            current_user,
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

/// Install the `window.OmnibusAudio` control surface immediately so
/// the transport buttons are wired even before `initDirect` / `initHls`
/// attaches. The two init paths are responsible for setting their own
/// initial position (per-part for direct, absolute for hls) via one-shot
/// `loadedmetadata` listeners.
fn install_control_surface(uuid: &str, initial_rate: f64, initial_volume: f64) {
    let rate_lit = serde_json::to_string(&initial_rate).unwrap_or_else(|_| "1".into());
    let vol_lit = serde_json::to_string(&initial_volume).unwrap_or_else(|_| "1".into());
    let uuid_lit = serde_json::to_string(uuid).unwrap_or_else(|_| "\"\"".into());
    let _ = dioxus::document::eval(&control_surface_js(&rate_lit, &vol_lit, &uuid_lit));
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
    user_id: Option<i64>,
    initial_position: f64,
    playback: crate::PlaybackState,
    current_user: Signal<Option<Option<omnibus_shared::UserSummary>>>,
) {
    let mut rate = playback.rate;
    let mut rate_error = playback.rate_error;
    let hls_ready = playback.hls_ready;
    let mut playback_failed = playback.playback_failed;
    let chapters_sig = playback.chapters;
    let uuid_guard = playback.uuid;
    // True only while `uuid_for_fetch` is still the active book. Checked before
    // every shared-signal write so a stale task (user switched books mid-fetch
    // or mid-`/status`-poll) can't clobber the new book's state.
    let is_current = || {
        let active_user_id = current_user
            .peek()
            .as_ref()
            .and_then(|user| user.as_ref())
            .map(|user| user.id);
        crate::audiobook_progress::playback_load_matches(
            uuid_guard.peek().as_deref(),
            active_user_id,
            &uuid_for_fetch,
            user_id,
        )
    };
    // Reconcile resume position with the server upfront so
    // both init paths see the same starting point.
    let server_pos = data::get_progress("", &uuid_for_fetch, omnibus_shared::ProgressFormat::Audio)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.audio_position_seconds);
    let resume_pos = resolve_resume_pos(server_pos, initial_position);
    let pos_lit = serde_json::to_string(&resume_pos).unwrap_or_else(|_| "0".into());

    let server_rate = data::get_playback_rate("", &uuid_for_fetch).await;
    if !is_current() {
        return;
    }
    if let (Some(user_id), Ok(Some(record))) = (user_id, server_rate.as_ref()) {
        crate::audiobook_progress::save_rate(user_id, &uuid_for_fetch, record.playback_rate);
    }
    let local_rate =
        user_id.and_then(|id| crate::audiobook_progress::load_rate(id, &uuid_for_fetch));
    let resolution = crate::audiobook_progress::resolve_rate(
        server_rate
            .as_ref()
            .map(|record| record.as_ref().map(|record| record.playback_rate))
            .map_err(|_| ()),
        local_rate,
    );
    rate.set(resolution.playback_rate);
    super::helpers::audio_call("setRate", &resolution.playback_rate.to_string());
    if resolution.seed_server {
        let update = omnibus_shared::AudiobookPlaybackRateUpdate {
            playback_rate: resolution.playback_rate,
        };
        if let Err(error) = data::set_playback_rate("", &uuid_for_fetch, update).await {
            if is_current() {
                rate_error.set(Some(format!("Could not save playback speed: {error}")));
            }
        }
    }

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
    fn needs_reload_when_nothing_booted_or_book_changed() {
        assert!(needs_reload(&None, "book-a", None));
        let booted = Some(("book-a".to_string(), Some(917)));
        assert!(needs_reload(&booted, "book-b", Some(940)));
    }

    #[test]
    fn needs_reload_on_explicit_different_part_of_same_book() {
        let booted = Some(("book-a".to_string(), Some(917)));
        assert!(needs_reload(&booted, "book-a", Some(918)));
    }

    #[test]
    fn no_reload_for_same_part_or_bare_relink_of_same_book() {
        let booted = Some(("book-a".to_string(), Some(917)));
        // Same part re-selected → no restart.
        assert!(!needs_reload(&booted, "book-a", Some(917)));
        // Mini-dock relinks with no file_id → keep the current part.
        assert!(!needs_reload(&booted, "book-a", None));
    }
}
