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

/// Render-less app-root component owning the load → install → drain pipeline.
#[component]
pub fn MobileAudioHost() -> Element {
    let ctx = use_mobile_playback();
    let server_url = crate::contexts::use_server_url_signal();
    // The live drain task, cancelled before every reinstall so a superseded
    // loop can't sit on a dead eval channel forever.
    let mut drain_task = use_signal(|| None::<Task>);
    let mut loaded_uuid = use_signal(|| None::<String>);

    // Loader: reacts to the listen page retargeting `ctx.uuid`.
    use_effect(move || {
        let requested = (ctx.uuid)();
        if requested == *loaded_uuid.peek() {
            return;
        }
        if let Some(task) = drain_task.write().take() {
            task.cancel();
        }
        loaded_uuid.set(requested.clone());
        match requested {
            Some(uuid) => {
                let server = server_url.peek().clone();
                let task = spawn(load_and_drain(ctx, uuid, server));
                drain_task.set(Some(task));
            }
            None => {
                interop::teardown();
                reset_playback(ctx);
            }
        }
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
    sleep.set(SleepState::Off);
}

/// Fetch metadata + manifest for `uuid`, seed the context, install the audio
/// control surface for direct-play books (or flag HLS as unsupported), then
/// drain its events until superseded.
async fn load_and_drain(ctx: MobilePlayback, uuid: String, server_url: String) {
    let MobilePlayback {
        mut view,
        mut loading,
        mut error,
        mut unsupported,
        mut duration,
        mut elapsed,
        mut playing,
        mut rate,
        mut sleep,
        ..
    } = ctx;
    loading.set(true);
    error.set(None);
    unsupported.set(false);
    view.set(None);
    playing.set(false);
    sleep.set(SleepState::Off);
    rate.set(crate::audiobook_progress::load_rate(&uuid));

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
    let file_id = book.book_files.first().map(|f| f.id);
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
            let resume = resolve_resume(&server_url, &uuid).await;
            view.set(Some(PlayerView::from_direct(
                &book,
                chapters,
                total_duration_seconds,
                parts.clone(),
            )));
            duration.set(total_duration_seconds);
            elapsed.set(resume);
            loading.set(false);
            let eval = interop::install_direct_surface(&server_url, &parts, resume, *rate.peek());
            drain_audio_events(eval, ctx, uuid, server_url).await;
        }
        AudiobookManifest::Hls { .. } => {
            // hls.js isn't bundled on mobile; don't fake playback.
            view.set(Some(PlayerView::from_hls(&book)));
            unsupported.set(true);
            loading.set(false);
        }
    }
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
