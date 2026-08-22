//! Web-only `OmnibusAudio` bootstrap: installs the JS control surface on
//! `window`, wires Rust callbacks for time / duration / play / pause, then
//! fetches the manifest and branches on `mode` (direct vs hls) to seed
//! playback. Everything here is web-feature gated; the callback closures live
//! in [`callbacks`], the manifest branch in [`manifest`], the JS in [`js`].

#![cfg(feature = "web")]

use dioxus::prelude::*;

use crate::data;

mod callbacks;
mod js;
mod manifest;

use callbacks::{register_js_callbacks, register_readiness_callbacks};
use js::{control_surface_js, inject_hls_script};
use manifest::run_manifest_init;

/// Owns the `Closure`s wired into `window.__omnibusOn*` callbacks. Held
/// across re-renders by `use_hook` so the closures outlive the effect
/// closure that registers them.
// Boxed as `Any`: the held closures now span two arities (`FnMut(f64)` and
// the persist-gated `FnMut(f64, bool)`), and the holder only exists to keep
// them alive for the surface's lifetime — nothing ever downcasts.
type JsCallbackHolder = std::rc::Rc<std::cell::RefCell<Vec<Box<dyn std::any::Any>>>>;

// The pure resume-decision helpers (`resolve_boot_file`,
// `resolve_boot_position`) live in `super::helpers` — this module is
// web-gated, so tests here never run under the crate's test matrix (which
// exercises the `server` + `mobile` features). The rest of this module is
// JS/WASM interop (`eval`, `window.*` callbacks).

/// Whether the driver must (re)load for a newly-requested `(uuid, file_id)`.
///
/// A different book always reloads. The *same* book reloads only on an
/// explicit, different file selection — so picking another part in the file
/// picker switches parts, while the mini-dock (which relinks with no
/// `file_id`) resumes the current part instead of restarting from the first
/// file. Mirrors `pages::listen::mobile::host::needs_reload`.
fn needs_reload(
    loaded: &Option<(String, Option<i64>, u32)>,
    uuid: &str,
    file_id: Option<i64>,
    epoch: u32,
) -> bool {
    match loaded {
        Some((loaded_uuid, loaded_file, loaded_epoch)) if loaded_uuid == uuid => {
            // A bumped epoch forces a same-book re-boot (resume + follow
            // re-resolution) without the dock ever unmounting.
            *loaded_epoch != epoch || (file_id.is_some() && file_id != *loaded_file)
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
    let mut loaded_key = use_hook(|| Signal::new(None::<(String, Option<i64>, u32)>));

    use_effect(move || {
        let resolved_user = current_user();
        // Auth may still be resolving on a fresh page load (`None` = unknown).
        // Booting now would capture `user_id = None`; the manifest task's
        // `is_current` guard then bails once `current_user` resolves to a real
        // id, leaving the player stuck "preparing". Wait for resolution —
        // this effect re-runs when `current_user` flips — instead of booting
        // early, because the `needs_reload` gate would otherwise suppress the
        // corrected re-boot.
        if resolved_user.is_none() {
            return;
        }
        if matches!(resolved_user, Some(None)) {
            let mut uuid = playback.uuid;
            uuid.set(None);
            return;
        }
        // Reactive dependencies — re-run when the active book *or* the selected
        // file changes (the picker retargets `file_id` without changing uuid),
        // or when a surface bumps `reload_epoch` to force a same-book re-boot
        // (Immersive Read re-resolving resume + follow without unmounting the
        // dock).
        let requested_uuid = playback.uuid.read().clone();
        let requested_file = *playback.file_id.read();
        let requested_epoch = *playback.reload_epoch.read();
        let Some(uuid) = requested_uuid else {
            // Dismissed via the dock × button. The handler already stopped
            // the element; clear the book so the dock hides everywhere, and
            // forget the booted key so re-selecting the same book reloads.
            let mut book = playback.book;
            book.set(None);
            loaded_key.set(None);
            return;
        };
        if !needs_reload(
            &loaded_key.peek().clone(),
            &uuid,
            requested_file,
            requested_epoch,
        ) {
            return;
        }
        loaded_key.set(Some((uuid.clone(), requested_file, requested_epoch)));
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
        playback.loaded_file_id,
        playback.duration,
        playback.elapsed,
        playback.playing,
        playback.playback_failed,
        playback.file_id,
        playback.uuid,
    );
    register_readiness_callbacks(
        cb_holder,
        playback.hls_ready,
        playback.buffering,
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
    let mut buffering = playback.buffering;
    let mut playback_failed = playback.playback_failed;
    NEXT_SEQUENCE_FILE.with(|c| c.set(None));
    let mut rate = playback.rate;
    let mut rate_error = playback.rate_error;
    let mut book = playback.book;
    let mut error = playback.error;
    let mut chapters = playback.chapters;
    let mut loaded_file_id = playback.loaded_file_id;

    loaded_file_id.set(None);
    duration.set(0.0_f64);
    elapsed.set(0.0_f64);
    playing.set(false);
    hls_ready.set(false);
    buffering.set(false);
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

thread_local! {
    /// The manifest's next-file pointer for the CURRENT boot — read by the
    /// `ended` callback (registered before any manifest exists) to decide
    /// between sequence-advance and marking the book finished. Reset at
    /// every boot entry so a failed load can't leave a stale advance.
    static NEXT_SEQUENCE_FILE: std::cell::Cell<Option<i64>> = const { std::cell::Cell::new(None) };
}

// Tests for the pure Rust-side logic only. The JS interop seams
// (register_js_callbacks, inject_hls_script, install_control_surface,
// run_manifest_init) require a WASM runtime and are covered by Playwright
// at ui_tests/playwright/tests/flows/.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn needs_reload_when_nothing_booted_or_book_changed() {
        assert!(needs_reload(&None, "book-a", None, 0));
        let booted = Some(("book-a".to_string(), Some(917), 0));
        assert!(needs_reload(&booted, "book-b", Some(940), 0));
    }

    #[test]
    fn needs_reload_on_explicit_different_part_of_same_book() {
        let booted = Some(("book-a".to_string(), Some(917), 0));
        assert!(needs_reload(&booted, "book-a", Some(918), 0));
    }

    #[test]
    fn needs_reload_when_the_reload_epoch_was_bumped() {
        // Immersive Read forces a same-book re-boot without unmounting the
        // dock — the epoch is the only thing that moved.
        let booted = Some(("book-a".to_string(), Some(917), 0));
        assert!(needs_reload(&booted, "book-a", None, 1));
        assert!(needs_reload(&booted, "book-a", Some(917), 1));
    }

    #[test]
    fn no_reload_for_same_part_or_bare_relink_of_same_book() {
        let booted = Some(("book-a".to_string(), Some(917), 0));
        // Same part re-selected → no restart.
        assert!(!needs_reload(&booted, "book-a", Some(917), 0));
        // Mini-dock relinks with no file_id → keep the current part.
        assert!(!needs_reload(&booted, "book-a", None, 0));
    }
}
