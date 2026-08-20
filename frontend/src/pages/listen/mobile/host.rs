//! App-root playback driver for mobile.
//!
//! Mounted once by [`crate::App`]; reacts to [`MobilePlayback::uuid`] by
//! loading the manifest, installing the JS `<audio>` surface, and draining
//! events into [`MobilePlayback`].

use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use omnibus_shared::{AudiobookManifest, ProgressFormat, ProgressRecord};

use super::state::{use_mobile_playback, MobilePlayback, SleepState};
use super::view::PlayerView;
use super::{interop, persist_position};
use crate::data;
use crate::pages::listen::helpers::{resolve_boot_file, resolve_boot_position};

/// `book_files.id` of the first audio file, lowest ordinal wins, or `None`.
///
/// A book with both an ebook and an audiobook lists the ebook format first
/// (`get_book_files` orders by format), so `book_files.first()` would hand the
/// audiobook manifest an EPUB id and 404. Shared with the offline download
/// engine (`offline::downloads::default_audio_file_id`) so download and
/// playback pick the same default file.
fn first_audio_file_id(files: &[omnibus_shared::BookFileInfo]) -> Option<i64> {
    crate::offline::downloads::default_audio_file_id(files)
}

/// The `book_files` id a manifest's `file_identity()` names, or `None` for
/// a server predating file-identity fields (`file_identity() == (0, 0)`).
/// Position writes must carry this — the file the manifest actually
/// resolved — rather than the raw requested id (#1888, #1923).
fn resolved_file_id(loaded_file: i64) -> Option<i64> {
    (loaded_file > 0).then_some(loaded_file)
}

/// Whether a manifest fetch that failed against the resolved boot file
/// should retry once against the client-computed default: only when the
/// picker made no explicit choice (a dead `?file_id=` must fail loudly,
/// never silently substitute another file) and the resolved file actually
/// differs from the default (nothing to gain retrying the same request).
/// Covers a stale progress-row `book_file_id` a reindex has since replaced.
fn should_retry_manifest_with_default(
    selected_file_id: Option<i64>,
    boot_file: Option<i64>,
    default_file: Option<i64>,
) -> bool {
    selected_file_id.is_none() && boot_file != default_file
}

/// What a load run is playing: the book, plus the `book_files` id every
/// position write should carry — the file the manifest actually resolved
/// (an explicit picker selection, else the progress row's stored
/// `book_file_id`, else the server's lowest-ordinal default), not the raw
/// `?file_id=` the route was entered with. Mirrors web's
/// `PlaybackState::loaded_file_id` (#1888, #1923). `None` only when the
/// server predates file-identity fields (`audio_file_count == 0`).
struct LoadTarget {
    uuid: String,
    file_id: Option<i64>,
}

/// Whether the host must (re)load for a newly-requested `(uuid, file_id)`.
///
/// A different book always reloads. The *same* book reloads only on an
/// explicit, different file selection — so tapping another part in the picker
/// switches files, while the mini-player (which links with no `file_id`)
/// resumes the current part instead of restarting from the first file.
fn needs_reload(loaded: &Option<(String, Option<i64>)>, uuid: &str, file_id: Option<i64>) -> bool {
    match loaded {
        Some((loaded_uuid, loaded_file)) if loaded_uuid == uuid => {
            file_id.is_some() && file_id != *loaded_file
        }
        _ => true,
    }
}

/// Render-less app-root component owning the load → install → drain pipeline.
#[component]
pub fn MobileAudioHost() -> Element {
    let ctx = use_mobile_playback();
    let server_url = crate::contexts::use_server_url_signal();
    // The live drain task, cancelled before every reinstall so a superseded
    // loop can't sit on a dead eval channel forever.
    let mut drain_task = use_signal(|| None::<Task>);
    // What's currently loaded: (book uuid, the file_id it was loaded with).
    let mut loaded_key = use_signal(|| None::<(String, Option<i64>)>);

    use_future(move || async move {
        let mut auth = crate::data::token_store::subscribe();
        while auth.changed().await.is_ok() {
            if !*auth.borrow_and_update() {
                let mut uuid = ctx.uuid;
                uuid.set(None);
            }
        }
    });

    // Record listening time off the play/pause signals — the rows behind
    // the `/stats` aggregates, POSTed to `/api/progress/sessions`.
    crate::session_tracker::use_listening_session(ctx.uuid, ctx.playing, server_url);

    // Loader: reacts to the listen page retargeting `ctx.uuid` / `ctx.file_id`.
    use_effect(move || {
        let requested_uuid = (ctx.uuid)();
        let requested_file = (ctx.file_id)();
        let Some(uuid) = requested_uuid else {
            // Nothing requested — tear down if a book was loaded.
            if loaded_key.peek().is_some() {
                if let Some(task) = drain_task.write().take() {
                    task.cancel();
                }
                loaded_key.set(None);
                interop::teardown();
                reset_playback(ctx);
            }
            return;
        };
        if !needs_reload(&loaded_key.peek().clone(), &uuid, requested_file) {
            return;
        }
        if let Some(task) = drain_task.write().take() {
            task.cancel();
        }
        loaded_key.set(Some((uuid.clone(), requested_file)));
        let server = server_url.peek().clone();
        let task = spawn(load_and_drain(ctx, uuid, requested_file, server));
        drain_task.set(Some(task));
    });

    // Wall-clock sleep countdown: a self-re-arming 1 s tick (the mobile
    // mirror of web `sleep::use_sleep_timer`). Each tick decrements only if
    // the state is still exactly what it armed against, so re-picks and
    // cancels orphan in-flight ticks instead of double-decrementing.
    use_effect(move || {
        let armed = (ctx.sleep)();
        let SleepState::Countdown { remaining, preset } = armed else {
            return;
        };
        let mut sleep = ctx.sleep;
        if remaining <= 0 {
            interop::pause();
            sleep.set(SleepState::Off);
            return;
        }
        spawn(async move {
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            if *sleep.peek() == armed {
                sleep.set(SleepState::Countdown {
                    remaining: remaining - 1,
                    preset,
                });
            }
        });
    });

    rsx! {}
}

/// Return every signal to the nothing-playing baseline.
fn reset_playback(ctx: MobilePlayback) {
    let MobilePlayback {
        mut view,
        mut loading,
        mut error,
        mut unsupported,
        mut duration,
        mut elapsed,
        mut playing,
        mut rate_error,
        mut user_id,
        mut sleep,
        ..
    } = ctx;
    view.set(None);
    loading.set(false);
    error.set(None);
    unsupported.set(false);
    duration.set(0.0);
    elapsed.set(0.0);
    playing.set(false);
    rate_error.set(None);
    user_id.set(None);
    sleep.set(SleepState::Off);
}

/// Fetch the book's metadata for `uuid`, surfacing "not found" or transport
/// errors on the `error`/`loading` signals. `None` means the caller should
/// return without proceeding — the failure is already reported.
async fn load_book_metadata(
    server_url: &str,
    uuid: &str,
    mut error: Signal<Option<String>>,
    mut loading: Signal<bool>,
) -> Option<omnibus_shared::EbookMetadata> {
    match data::get_ebook(server_url, uuid).await {
        Ok(Some(b)) => Some(b),
        Ok(None) => {
            error.set(Some("Audiobook not found.".into()));
            loading.set(false);
            None
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            loading.set(false);
            None
        }
    }
}

/// Resolve the boot file the same way the web bootstrap's
/// `resolve_boot_file` does — an explicit picker selection wins, otherwise
/// the progress row's stored `book_file_id`, so resume lands in the file
/// the seconds were recorded in (#1888) — then fetch its manifest,
/// surfacing transport errors on `error`/`loading`. If neither names one,
/// falls back to the first audio file (mirrors the server's own
/// lowest-ordinal default).
///
/// The row's `book_file_id` is a soft reference: a reindex may have
/// replaced the `book_files` row since the position was saved. A failed
/// fetch against it retries once against the client-computed default
/// rather than failing an otherwise-healthy book — never when the picker
/// made an explicit choice, since a dead `?file_id=` should fail loudly.
/// Returns the manifest and the progress row consulted (reused by
/// `init_direct_and_drain` for the resume position, avoiding a second
/// fetch). `None` on failure — the failure is already reported.
async fn load_manifest(
    server_url: &str,
    uuid: &str,
    selected_file_id: Option<i64>,
    book: &omnibus_shared::EbookMetadata,
    mut error: Signal<Option<String>>,
    mut loading: Signal<bool>,
) -> Option<(AudiobookManifest, Option<ProgressRecord>)> {
    let row = data::get_progress(server_url, uuid, ProgressFormat::Audio)
        .await
        .ok()
        .flatten();
    let row_file = row.as_ref().and_then(|r| r.book_file_id);
    let default_file = first_audio_file_id(&book.book_files);
    let boot_file = resolve_boot_file(selected_file_id, row_file).or(default_file);
    match data::get_manifest(server_url, uuid, boot_file).await {
        Ok(m) => Some((m, row)),
        Err(_) if should_retry_manifest_with_default(selected_file_id, boot_file, default_file) => {
            match data::get_manifest(server_url, uuid, default_file).await {
                Ok(m) => Some((m, row)),
                Err(e) => {
                    error.set(Some(e.to_string()));
                    loading.set(false);
                    None
                }
            }
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            loading.set(false);
            None
        }
    }
}

/// Fetch metadata + manifest for `uuid`, seed the context, install the audio
/// control surface for direct-play books (or flag HLS as unsupported), then
/// drain its events until superseded.
async fn load_and_drain(
    ctx: MobilePlayback,
    uuid: String,
    selected_file_id: Option<i64>,
    server_url: String,
) {
    let MobilePlayback {
        mut view,
        mut loading,
        mut error,
        mut unsupported,
        mut playing,
        mut rate,
        mut rate_error,
        mut sleep,
        ..
    } = ctx;
    loading.set(true);
    error.set(None);
    unsupported.set(false);
    view.set(None);
    playing.set(false);
    rate_error.set(None);
    sleep.set(SleepState::Off);
    rate.set(1.0);

    let Some(book) = load_book_metadata(&server_url, &uuid, error, loading).await else {
        return;
    };
    let Some((manifest, row)) =
        load_manifest(&server_url, &uuid, selected_file_id, &book, error, loading).await
    else {
        return;
    };

    // The file the manifest actually resolved — `0`/`0` means a server
    // predating file-identity fields; keep posting no file id rather than
    // inventing one (mirrors web's `run_manifest_init`).
    let (loaded_file, audio_file_count) = manifest.file_identity();
    let loaded_file_id = resolved_file_id(loaded_file);

    match manifest {
        AudiobookManifest::Direct {
            parts,
            total_duration_seconds,
            chapters,
            ..
        } => {
            // Same fail-fast as the reader's offline guard: with no completed
            // audio download, the <audio> element would stall against the
            // dead server instead of surfacing an error.
            if audio_blocked_offline(&uuid) {
                error.set(Some(
                    "You're offline — download this audiobook to listen offline.".into(),
                ));
                loading.set(false);
                return;
            }
            init_direct_and_drain(
                ctx,
                &book,
                LoadTarget {
                    uuid,
                    file_id: loaded_file_id,
                },
                row,
                loaded_file,
                audio_file_count,
                server_url,
                parts,
                total_duration_seconds,
                chapters,
            )
            .await;
        }
        AudiobookManifest::Hls { .. } => {
            // hls.js isn't bundled on mobile; don't fake playback.
            view.set(Some(PlayerView::from_hls(&book)));
            unsupported.set(true);
            loading.set(false);
        }
    }
}

/// Whether the transport icon (`playing`) disagrees with the element's real
/// `paused` state and must be flipped. `playing` should always be the negation
/// of `paused`; equality means they've drifted (a missed `play`/`pause` event).
fn playing_out_of_sync(playing: bool, paused: bool) -> bool {
    playing == paused
}

/// `true` when direct playback of `uuid` is doomed: the app is known-offline
/// and no completed audio download exists to serve the parts locally.
fn audio_blocked_offline(uuid: &str) -> bool {
    crate::offline::sync::is_offline()
        && !crate::offline::downloads::is_complete(uuid, crate::offline::downloads::DlFormat::Audio)
}

/// Resolve the effective playback rate: publish `user_id`, reconcile the
/// server-saved rate with the local cache, seed the server when only a local
/// value exists, and set the `rate` signal. Returns the resolved rate.
async fn resolve_playback_rate(ctx: MobilePlayback, server_url: &str, uuid: &str) -> f64 {
    let mut user_id_sig = ctx.user_id;
    let mut rate = ctx.rate;
    let mut rate_error = ctx.rate_error;

    let resolved_user_id = data::get_me(server_url).await.ok().map(|user| user.id);
    user_id_sig.set(resolved_user_id);
    let server_rate = data::get_playback_rate(server_url, uuid).await;
    if let (Some(user_id), Ok(Some(record))) = (resolved_user_id, server_rate.as_ref()) {
        crate::audiobook_progress::save_rate(user_id, uuid, record.playback_rate);
    }
    let local_rate =
        resolved_user_id.and_then(|user_id| crate::audiobook_progress::load_rate(user_id, uuid));
    let resolution = crate::audiobook_progress::resolve_rate(
        server_rate
            .as_ref()
            .map(|record| record.as_ref().map(|record| record.playback_rate))
            .map_err(|_| ()),
        local_rate,
    );
    rate.set(resolution.playback_rate);
    if resolution.seed_server {
        let update = omnibus_shared::AudiobookPlaybackRateUpdate {
            playback_rate: resolution.playback_rate,
        };
        if let Err(error) = data::set_playback_rate(server_url, uuid, update).await {
            rate_error.set(Some(format!("Could not save playback speed: {error}")));
        }
    }
    resolution.playback_rate
}

/// Direct-play arm of [`load_and_drain`]: resolve resume + rate, seed the
/// view/duration signals, install the JS control surface, then drain audio
/// events until superseded.
#[allow(clippy::too_many_arguments)] // orthogonal resume/manifest inputs; a bundling struct would just rename them
async fn init_direct_and_drain(
    ctx: MobilePlayback,
    book: &omnibus_shared::EbookMetadata,
    target: LoadTarget,
    row: Option<ProgressRecord>,
    loaded_file: i64,
    audio_file_count: i64,
    server_url: String,
    parts: Vec<omnibus_shared::ManifestPart>,
    total_duration_seconds: f64,
    chapters: Vec<omnibus_shared::ChapterInfo>,
) {
    let mut view = ctx.view;
    let mut loading = ctx.loading;
    let mut duration = ctx.duration;
    let mut elapsed = ctx.elapsed;
    let LoadTarget { uuid, file_id } = target;

    // Mirrors web's `resolve_boot_position`: on a multi-file book, seconds
    // apply only when the row names the file that was actually loaded —
    // otherwise they'd splice one file's offset into another (#1888).
    let local_pos = crate::audiobook_progress::load(&uuid).unwrap_or(0.0);
    // `None` (multi-file row the loaded file can't claim) boots at 0 here:
    // this surface has no unseeded-session gate — its persistence only fires
    // from real playback events in `drain_audio_events`, so a 0 boot can't
    // be flushed back the way the web teardown path could.
    let resume = resolve_boot_position(
        row.map(|r| (r.book_file_id, r.audio_position_seconds)),
        local_pos,
        loaded_file,
        audio_file_count,
    )
    .unwrap_or(0.0);
    let playback_rate = resolve_playback_rate(ctx, &server_url, &uuid).await;
    let pv = PlayerView::from_direct(book, chapters, total_duration_seconds, parts.clone());
    // Cover artwork for the lock screen: the same tokened thumbnail the
    // hero uses. WebKit fetches it itself, so it must carry `?token=`.
    let artwork = book
        .cover_url
        .as_ref()
        .map(|_| crate::thumb_url(&server_url, &uuid, "lg"));
    let now_playing = interop::NowPlaying {
        title: &pv.title,
        author: &pv.author,
        artwork_url: artwork.as_deref(),
    };
    duration.set(total_duration_seconds);
    elapsed.set(resume);
    loading.set(false);
    let eval = interop::install_direct_surface(
        &server_url,
        &uuid,
        &parts,
        resume,
        playback_rate,
        &now_playing,
    );
    view.set(Some(pv));
    drain_audio_events(eval, ctx, LoadTarget { uuid, file_id }, server_url).await;
}

/// Drain the JS→Rust audio event channel until it closes (surface torn
/// down), updating the position / playing signals, throttling position
/// persistence to ~5 s deltas, and firing the armed end-of-chapter sleep
/// boundary. The drain loop itself is [`crate::js_interop::drain_events`],
/// shared with the barcode scanner and mobile reader interop.
async fn drain_audio_events(
    eval: dioxus::document::Eval,
    ctx: MobilePlayback,
    target: LoadTarget,
    server_url: String,
) {
    let MobilePlayback {
        mut elapsed,
        mut playing,
        mut sleep,
        ..
    } = ctx;
    let LoadTarget { uuid, file_id } = target;
    let mut last_saved = 0.0_f64;
    // One `Reading` write per loaded book: `Play` also fires on every resume
    // from pause, and a file switch re-runs this drain with a fresh flag.
    let mut marked_reading = false;
    crate::js_interop::drain_events(eval, move |event: interop::AudioEvent| match event {
        interop::AudioEvent::Time { seconds, paused } => {
            elapsed.set(seconds);
            // WKWebView can resume after an interruption without re-firing `play`; reconcile the icon.
            if playing_out_of_sync(*playing.peek(), paused) {
                playing.set(!paused);
            }
            let armed = *sleep.peek();
            if let SleepState::EndOfChapter { at_seconds } = armed {
                if seconds >= at_seconds {
                    interop::pause();
                    sleep.set(SleepState::Off);
                }
            }
            if (seconds - last_saved).abs() >= 5.0 {
                last_saved = seconds;
                persist_position(&uuid, file_id, &server_url, seconds);
            }
        }
        interop::AudioEvent::Play => {
            playing.set(true);
            if !marked_reading {
                marked_reading = true;
                mark_read_status(&uuid, &server_url, false);
            }
        }
        interop::AudioEvent::Pause { seconds } => {
            playing.set(false);
            elapsed.set(seconds);
            persist_position(&uuid, file_id, &server_url, seconds);
        }
        interop::AudioEvent::Ended => {
            playing.set(false);
            mark_read_status(&uuid, &server_url, true);
        }
    })
    .await;
}

/// Spawn the best-effort auto read-status write for the playing book.
/// `at_end` is false on the first play (`Unread` → `Reading`) and true once
/// every file has played out (→ `Finished`).
fn mark_read_status(uuid: &str, server_url: &str, at_end: bool) {
    let uuid = uuid.to_string();
    let server_url = server_url.to_string();
    spawn(async move {
        crate::read_status_auto::apply_auto_read_status(&server_url, &uuid, at_end).await;
    });
}

#[cfg(test)]
mod tests {
    use super::{
        audio_blocked_offline, first_audio_file_id, playing_out_of_sync, resolved_file_id,
        should_retry_manifest_with_default,
    };
    use omnibus_shared::BookFileInfo;

    #[test]
    fn playing_out_of_sync_flags_only_drifted_state() {
        // Agreement: playing == !paused — nothing to reconcile.
        assert!(!playing_out_of_sync(true, false));
        assert!(!playing_out_of_sync(false, true));
        // Drift: the icon and the element disagree and must be flipped.
        assert!(playing_out_of_sync(false, false));
        assert!(playing_out_of_sync(true, true));
    }

    #[test]
    fn audio_blocked_offline_only_while_offline_without_a_download() {
        let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
        crate::offline::sync::note_offline();
        assert!(
            audio_blocked_offline("u-host-guard"),
            "offline with no completed audio download must block playback"
        );
        crate::offline::sync::note_online();
        assert!(
            !audio_blocked_offline("u-host-guard"),
            "online playback is never blocked"
        );
    }

    fn bf(id: i64, format: &str, ordinal: i64) -> BookFileInfo {
        BookFileInfo {
            id,
            format: format.into(),
            filename: String::new(),
            ordinal,
            label: None,
            size_bytes: 0,
            path: None,
            etag: None,
        }
    }

    #[test]
    fn first_audio_file_id_skips_a_leading_ebook_format() {
        // A merged ebook+audiobook lists EPUB first (ordered by format); the
        // audiobook manifest must still receive the M4B id, not the EPUB's.
        let files = vec![bf(698, "EPUB", 0), bf(917, "M4B", 0), bf(922, "M4B", 1)];
        assert_eq!(first_audio_file_id(&files), Some(917));
    }

    #[test]
    fn first_audio_file_id_picks_lowest_ordinal() {
        let files = vec![bf(5, "M4B", 2), bf(3, "M4B", 0), bf(4, "M4B", 1)];
        assert_eq!(first_audio_file_id(&files), Some(3));
    }

    #[test]
    fn first_audio_file_id_is_none_without_an_audio_file() {
        let files = vec![bf(1, "EPUB", 0), bf(2, "PDF", 0)];
        assert_eq!(first_audio_file_id(&files), None);
    }

    #[test]
    fn resolved_file_id_names_a_positive_loaded_file() {
        assert_eq!(resolved_file_id(917), Some(917));
    }

    #[test]
    fn resolved_file_id_is_none_for_a_legacy_server_without_identity() {
        // `file_identity()` decodes a pre-#1888 payload as (0, 0); posting a
        // fabricated id would be worse than posting none at all.
        assert_eq!(resolved_file_id(0), None);
    }

    #[test]
    fn should_retry_manifest_with_default_only_on_an_unattributed_fallback() {
        // No explicit selection, and the row's (stale) file differs from
        // the client-computed default — worth one retry.
        assert!(should_retry_manifest_with_default(
            None,
            Some(919),
            Some(917)
        ));
    }

    #[test]
    fn should_retry_manifest_with_default_never_overrides_an_explicit_pick() {
        // A picker selection failing must fail loudly, not silently swap
        // in a different file the listener didn't ask for.
        assert!(!should_retry_manifest_with_default(
            Some(919),
            Some(919),
            Some(917)
        ));
    }

    #[test]
    fn should_retry_manifest_with_default_skips_when_already_the_default() {
        // The failed request already *was* the default — retrying it would
        // just repeat the same failing call.
        assert!(!should_retry_manifest_with_default(
            None,
            Some(917),
            Some(917)
        ));
        assert!(!should_retry_manifest_with_default(None, None, None));
    }

    use super::needs_reload;

    #[test]
    fn needs_reload_when_nothing_loaded_or_book_changed() {
        assert!(needs_reload(&None, "book-a", None));
        let loaded = Some(("book-a".to_string(), Some(917)));
        assert!(needs_reload(&loaded, "book-b", Some(940)));
    }

    #[test]
    fn needs_reload_on_explicit_different_part_of_same_book() {
        let loaded = Some(("book-a".to_string(), Some(917)));
        assert!(needs_reload(&loaded, "book-a", Some(918)));
    }

    #[test]
    fn no_reload_for_same_part_or_bare_relink_of_same_book() {
        let loaded = Some(("book-a".to_string(), Some(917)));
        // Same part re-selected → no restart.
        assert!(!needs_reload(&loaded, "book-a", Some(917)));
        // Mini-player relinks with no file_id → keep the current part.
        assert!(!needs_reload(&loaded, "book-a", None));
    }
}
