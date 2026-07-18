//! App-root playback driver for mobile.
//!
//! Mounted once by [`crate::App`]; reacts to [`MobilePlayback::uuid`] by
//! loading the manifest, installing the JS `<audio>` surface, and draining
//! events into [`MobilePlayback`].

use dioxus::dioxus_core::Task;
use dioxus::prelude::*;
use omnibus_shared::AudiobookManifest;

use super::state::{use_mobile_playback, MobilePlayback, SleepState};
use super::view::PlayerView;
use super::{interop, persist_position, resolve_resume};
use crate::data;

/// `book_files.id` of the first audio file, lowest ordinal wins, or `None`.
///
/// A book with both an ebook and an audiobook lists the ebook format first
/// (`get_book_files` orders by format), so `book_files.first()` would hand the
/// audiobook manifest an EPUB id and 404. Filter to the audio formats the HLS
/// resolver accepts (`db::hls::resolve_audiobook`'s `format IN
/// ('M4B','M4A','MP3')`) before picking.
fn first_audio_file_id(files: &[omnibus_shared::BookFileInfo]) -> Option<i64> {
    files
        .iter()
        .filter(|f| {
            f.format.eq_ignore_ascii_case("M4B")
                || f.format.eq_ignore_ascii_case("M4A")
                || f.format.eq_ignore_ascii_case("MP3")
        })
        .min_by_key(|f| f.ordinal)
        .map(|f| f.id)
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

    let book = match data::get_ebook(&server_url, &uuid).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            error.set(Some("Audiobook not found.".into()));
            loading.set(false);
            return;
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            loading.set(false);
            return;
        }
    };
    // Honor the picker's explicit choice; otherwise default to the first audio
    // file (a bare `/listen/:uuid` with no `?file_id=`).
    let file_id = selected_file_id.or_else(|| first_audio_file_id(&book.book_files));
    let manifest = match data::get_manifest(&server_url, &uuid, file_id).await {
        Ok(m) => m,
        Err(e) => {
            error.set(Some(e.to_string()));
            loading.set(false);
            return;
        }
    };

    match manifest {
        AudiobookManifest::Direct {
            parts,
            total_duration_seconds,
            chapters,
        } => {
            init_direct_and_drain(
                ctx,
                &book,
                uuid,
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
async fn init_direct_and_drain(
    ctx: MobilePlayback,
    book: &omnibus_shared::EbookMetadata,
    uuid: String,
    server_url: String,
    parts: Vec<omnibus_shared::ManifestPart>,
    total_duration_seconds: f64,
    chapters: Vec<omnibus_shared::ChapterInfo>,
) {
    let mut view = ctx.view;
    let mut loading = ctx.loading;
    let mut duration = ctx.duration;
    let mut elapsed = ctx.elapsed;

    let resume = resolve_resume(&server_url, &uuid).await;
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
    let eval =
        interop::install_direct_surface(&server_url, &parts, resume, playback_rate, &now_playing);
    view.set(Some(pv));
    drain_audio_events(eval, ctx, uuid, server_url).await;
}

/// Drain the JS→Rust audio event channel until cancelled, updating the
/// position / playing signals, throttling position persistence to ~5 s
/// deltas, and firing the armed end-of-chapter sleep boundary.
async fn drain_audio_events(
    mut eval: dioxus::document::Eval,
    ctx: MobilePlayback,
    uuid: String,
    server_url: String,
) {
    let MobilePlayback {
        mut elapsed,
        mut playing,
        mut sleep,
        ..
    } = ctx;
    let mut last_saved = 0.0_f64;
    loop {
        match eval.recv::<interop::AudioEvent>().await {
            Ok(interop::AudioEvent::Time { seconds }) => {
                elapsed.set(seconds);
                let armed = *sleep.peek();
                if let SleepState::EndOfChapter { at_seconds } = armed {
                    if seconds >= at_seconds {
                        interop::pause();
                        sleep.set(SleepState::Off);
                    }
                }
                if (seconds - last_saved).abs() >= 5.0 {
                    last_saved = seconds;
                    persist_position(&uuid, &server_url, seconds);
                }
            }
            Ok(interop::AudioEvent::Play) => playing.set(true),
            Ok(interop::AudioEvent::Pause { seconds }) => {
                playing.set(false);
                elapsed.set(seconds);
                persist_position(&uuid, &server_url, seconds);
            }
            // Channel closed (surface torn down) — stop draining.
            Err(_) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::first_audio_file_id;
    use omnibus_shared::BookFileInfo;

    fn bf(id: i64, format: &str, ordinal: i64) -> BookFileInfo {
        BookFileInfo {
            id,
            format: format.into(),
            filename: String::new(),
            ordinal,
            label: None,
            size_bytes: 0,
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
