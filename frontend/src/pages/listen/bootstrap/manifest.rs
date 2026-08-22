//! Manifest-driven boot: reconcile rate + resume with the server, fetch
//! `/api/audiobooks/{uuid}/manifest`, then branch on `mode` — hand the
//! parts to `initDirect`, or poll the HLS transcode until it is ready.

use dioxus::prelude::*;

use super::super::helpers::{resolve_boot_file, resolve_boot_position};
use super::js::eval_hls_init;
use super::NEXT_SEQUENCE_FILE;
use crate::data;

/// Fetch `/api/audiobooks/{uuid}/manifest` — targeting the picker's
/// `file_id` when one was selected, else the progress row's stored
/// `book_file_id` so multi-file books resume in the file the seconds were
/// recorded in (#1888) — then either:
/// * **Direct mode** — call `initDirect` with the parts list (instant
///   playback for m4b/m4a/mp3/aac). `hls_ready` flips once the JS side
///   confirms via `__omnibusOnAudioBooted` that a source is actually
///   committed, not optimistically here.
/// * **HLS mode** — poll `/status` until `ready` (call `initHls`, same
///   JS-confirmed `hls_ready` flip) or `failed` (flip `playback_failed`).
/// * **No manifest** — show the same failure overlay as a terminal HLS
///   transcode failure.
pub(super) async fn run_manifest_init(
    uuid_for_fetch: String,
    file_id: Option<i64>,
    user_id: Option<i64>,
    initial_position: f64,
    playback: crate::PlaybackState,
    current_user: Signal<Option<Option<omnibus_shared::UserSummary>>>,
) {
    let mut playback_failed = playback.playback_failed;
    let chapters_sig = playback.chapters;
    let uuid_guard = playback.uuid;
    let is_current = || boot_is_current(current_user, uuid_guard, &uuid_for_fetch, user_id);
    // Reconcile resume state with the server upfront: the row carries both
    // the seconds and the `book_files` row they were recorded in, and the
    // file decides which manifest to fetch before any position applies.
    let server_row = data::get_progress("", &uuid_for_fetch, omnibus_shared::ProgressFormat::Audio)
        .await
        .ok()
        .flatten();

    if !reconcile_playback_rate(&uuid_for_fetch, user_id, playback, &is_current).await {
        return;
    }

    let row_file = server_row.as_ref().and_then(|r| r.book_file_id);
    let Some((follow_boot, boot_file)) =
        resolve_boot_target(&uuid_for_fetch, file_id, row_file, &is_current).await
    else {
        return;
    };
    let manifest = fetch_boot_manifest(&uuid_for_fetch, file_id, boot_file).await;

    // A newer book may have been selected while the manifest was in flight.
    // The Direct + None arms below are synchronous, so this single guard covers
    // them; the HLS arm re-checks each poll iteration.
    if !is_current() {
        return;
    }

    let Some(manifest) = manifest else {
        // Manifest unreachable (network failure, 5xx,
        // 404 between settings save and reindex). Show
        // the same failure overlay as a terminal HLS
        // transcode failure — a manual refresh is the
        // recovery path either way.
        playback_failed.set(true);
        return;
    };

    let (loaded_file, audio_file_count) = publish_boot_identity(&manifest, playback);
    let resume_pos = boot_resume_position(
        follow_boot,
        server_row,
        initial_position,
        loaded_file,
        audio_file_count,
    );
    let pos_lit = serde_json::to_string(&resume_pos).unwrap_or_else(|_| "null".into());

    match manifest {
        omnibus_shared::AudiobookManifest::Direct {
            parts, chapters, ..
        } => init_direct_play(parts, chapters, &pos_lit, chapters_sig),
        omnibus_shared::AudiobookManifest::Hls { playlist_url, .. } => {
            init_hls(
                &uuid_for_fetch,
                &playlist_url,
                &pos_lit,
                is_current,
                playback_failed,
            )
            .await;
        }
    }
}

/// Whether `uuid_for_fetch` is still the active book for the active user.
/// Checked before every shared-signal write so a stale task (user switched
/// books mid-fetch or mid-`/status`-poll) can't clobber the new book's
/// state.
fn boot_is_current(
    current_user: Signal<Option<Option<omnibus_shared::UserSummary>>>,
    uuid_guard: Signal<Option<String>>,
    uuid_for_fetch: &str,
    user_id: Option<i64>,
) -> bool {
    let active_user_id = current_user
        .peek()
        .as_ref()
        .and_then(|user| user.as_ref())
        .map(|user| user.id);
    crate::audiobook_progress::playback_load_matches(
        uuid_guard.peek().as_deref(),
        active_user_id,
        uuid_for_fetch,
        user_id,
    )
}

/// Resolve follow at boot: when the link follows the counterpart and the
/// reading position maps newer, seed playback with the mapped spot
/// instead of the stored row — a post-boot corrective seek would race
/// `initDirect`'s one-shot restore seek and lose. Fetched even for an
/// explicit `?file_id=` — every Continue card carries the row's file id,
/// and skipping follow there reopened books at the stale spot (#1972);
/// `resolve_follow_boot` still stands aside when the pick names a
/// *different* file than the candidate maps to. `None` when a newer book
/// was selected mid-fetch.
async fn resolve_boot_target(
    uuid_for_fetch: &str,
    file_id: Option<i64>,
    row_file: Option<i64>,
    is_current: &impl Fn() -> bool,
) -> Option<(Option<(Option<i64>, f64)>, Option<i64>)> {
    let follow_resume =
        super::super::sync_prompt::fetch_resume_with(uuid_for_fetch, "audio", false).await;
    if !is_current() {
        return None;
    }
    let follow_boot = super::super::helpers::resolve_follow_boot(file_id, follow_resume.as_ref());
    let boot_file = follow_boot
        .as_ref()
        .and_then(|(file, _)| *file)
        .or_else(|| resolve_boot_file(file_id, row_file));
    Some((follow_boot, boot_file))
}

/// Reconcile the playback rate with the server (seeding it when this
/// device's local preference is the only one) and apply it to the shim.
/// Returns `false` when a newer book was selected mid-fetch, meaning the
/// caller must abandon this boot.
async fn reconcile_playback_rate(
    uuid_for_fetch: &str,
    user_id: Option<i64>,
    playback: crate::PlaybackState,
    is_current: &impl Fn() -> bool,
) -> bool {
    let mut rate = playback.rate;
    let mut rate_error = playback.rate_error;
    let server_rate = data::get_playback_rate("", uuid_for_fetch).await;
    if !is_current() {
        return false;
    }
    if let (Some(user_id), Ok(Some(record))) = (user_id, server_rate.as_ref()) {
        crate::audiobook_progress::save_rate(user_id, uuid_for_fetch, record.playback_rate);
    }
    let local_rate =
        user_id.and_then(|id| crate::audiobook_progress::load_rate(id, uuid_for_fetch));
    let resolution = crate::audiobook_progress::resolve_rate(
        server_rate
            .as_ref()
            .map(|record| record.as_ref().map(|record| record.playback_rate))
            .map_err(|_| ()),
        local_rate,
    );
    rate.set(resolution.playback_rate);
    super::super::helpers::audio_call("setRate", &resolution.playback_rate.to_string());
    if resolution.seed_server {
        let update = omnibus_shared::AudiobookPlaybackRateUpdate {
            playback_rate: resolution.playback_rate,
        };
        if let Err(error) = data::set_playback_rate("", uuid_for_fetch, update).await {
            if is_current() {
                rate_error.set(Some(format!("Could not save playback speed: {error}")));
            }
        }
    }
    true
}

/// The boot manifest for `boot_file`. The row's stored id is a soft
/// reference — a reindex may have replaced the `book_files` row since the
/// position was saved. Fall back to the server's default file rather than
/// a failure overlay, mirroring `db::progress::resume_points`. Never
/// applied to an explicit picker selection: a dead `?file_id=` should fail
/// loudly, not play another file.
async fn fetch_boot_manifest(
    uuid_for_fetch: &str,
    file_id: Option<i64>,
    boot_file: Option<i64>,
) -> Option<omnibus_shared::AudiobookManifest> {
    let manifest = fetch_manifest(uuid_for_fetch, boot_file).await;
    if manifest.is_none() && file_id.is_none() && boot_file.is_some() {
        return fetch_manifest(uuid_for_fetch, None).await;
    }
    manifest
}

/// Publish what the manifest resolved: the sequence auto-advance pointer
/// and the loaded file id. Returns `(loaded_file, audio_file_count)`.
fn publish_boot_identity(
    manifest: &omnibus_shared::AudiobookManifest,
    playback: crate::PlaybackState,
) -> (i64, i64) {
    let (loaded_file, audio_file_count) = manifest.file_identity();
    // Sequence auto-advance is a DIRECT-mode contract only: the HLS
    // playlist is book-level and file-blind (`get_audiobook_playlist`
    // takes no file id), so "advancing" an HLS book reboots the same
    // stream at 0:00 forever instead of finishing it. HLS keeps the
    // ended-means-finished behavior.
    let next_file = match manifest {
        omnibus_shared::AudiobookManifest::Direct { .. } => manifest.next_file_id(),
        omnibus_shared::AudiobookManifest::Hls { .. } => None,
    };
    NEXT_SEQUENCE_FILE.with(|c| c.set(next_file));
    // `0` = a server predating the identity fields; keep posting no file id
    // rather than inventing one (`resolve_boot_position` degrades the same
    // way via `audio_file_count == 0`).
    let mut loaded_file_sig = playback.loaded_file_id;
    loaded_file_sig.set((loaded_file > 0).then_some(loaded_file));
    (loaded_file, audio_file_count)
}

/// The position this boot seeds playback with. `None` = unseeded:
/// `initDirect` gets a JSON `null`, applies no seek, and leaves the
/// transport's seed gate closed — an unattributable boot must stay
/// unpersistable until the user really plays or seeks.
fn boot_resume_position(
    follow_boot: Option<(Option<i64>, f64)>,
    server_row: Option<omnibus_shared::ProgressRecord>,
    initial_position: f64,
    loaded_file: i64,
    audio_file_count: i64,
) -> Option<f64> {
    // The mapped position applies only when the manifest actually loaded the
    // candidate's file (the dead-row fallback above may have swapped it) —
    // otherwise fall back to the stored-row resolution unchanged.
    let follow_pos = follow_boot
        .as_ref()
        .filter(|(file, _)| match file {
            Some(fid) => *fid == loaded_file || loaded_file == 0,
            None => true,
        })
        .map(|(_, seconds)| *seconds);
    follow_pos.or_else(|| {
        resolve_boot_position(
            server_row
                .as_ref()
                .map(|r| (r.book_file_id, r.audio_position_seconds)),
            initial_position,
            loaded_file,
            audio_file_count,
        )
    })
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

/// Direct-mode arm: hand the part list to the JS `initDirect` shim and
/// publish the chapter map. Synchronous — the caller's `is_current` guard
/// covered the only `await`. `hls_ready` is no longer flipped here — the
/// JS shim reports back via `__omnibusOnAudioBooted` once it has actually
/// committed a source, closing the race where a click could fire
/// `el.play()` before `el.src` existed.
fn init_direct_play(
    parts: Vec<omnibus_shared::ManifestPart>,
    chapters: Vec<omnibus_shared::ChapterInfo>,
    pos_lit: &str,
    mut chapters_sig: Signal<Vec<omnibus_shared::ChapterInfo>>,
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
}

/// HLS arm: poll `/api/audiobooks/{uuid}/status` until `ready` (call
/// `initHls`, which reports back via `__omnibusOnAudioBooted` the same way
/// the direct-mode arm does) or `failed` (flip `playback_failed`).
/// Re-checks `is_current` every iteration so a stale poll can't clobber
/// the newly-active book's signals.
async fn init_hls(
    uuid: &str,
    playlist_url: &str,
    pos_lit: &str,
    is_current: impl Fn() -> bool,
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
