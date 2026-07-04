//! Mobile audiobook player — the single-column phone adaptation of the
//! desktop two-column "Now playing" surface.
//!
//! Fetches `GET /api/audiobooks/{uuid}/manifest` on mount, drives an HTML
//! `<audio>` element (via `dioxus::document::eval`, since mobile is a wry
//! WebView) for direct-play books, and persists/restores position + rate
//! through [`crate::audiobook_progress`]. HLS manifests render an
//! unsupported state rather than faking playback.

#![cfg(feature = "mobile")]

use dioxus::prelude::*;
use omnibus_shared::{
    AudiobookManifest, ChapterInfo, EbookMetadata, ProgressFormat, ProgressUpdate,
};

use crate::components::atrium::Cover;
use crate::contexts::use_server_url;
use crate::data;
use crate::Route;
use dioxus_router::Link;

mod interop;
mod view;

use view::{chapter_index_for_elapsed, format_hms, format_ms, PlayerView};

/// Renders the mobile audiobook player for `uuid`. Owns the manifest fetch,
/// the `<audio>` control surface, and the position/rate persistence loop.
#[component]
pub fn MobilePlayer(uuid: String) -> Element {
    let server_url = use_server_url();

    // Playback + view state. All hooks declared unconditionally (rule 07).
    let view = use_signal(|| None::<PlayerView>);
    let error = use_signal(|| None::<String>);
    let unsupported = use_signal(|| false);
    let elapsed = use_signal(|| 0.0_f64);
    let duration = use_signal(|| 0.0_f64);
    let playing = use_signal(|| false);
    let mut rate = use_signal(|| crate::audiobook_progress::load_rate(&uuid));
    let chapters_open = use_signal(|| false);

    // Load book metadata + manifest on mount (re-run if the uuid changes).
    let load_uuid = uuid.clone();
    let load_server = server_url.clone();
    use_effect(use_reactive!(|(load_uuid, load_server)| {
        spawn(load_player(LoadCtx {
            uuid: load_uuid.clone(),
            server_url: load_server.clone(),
            view,
            error,
            unsupported,
            duration,
            elapsed,
            playing,
            rate,
        }));
    }));

    if let Some(msg) = error.read().clone() {
        return render_error(&msg);
    }
    if unsupported() {
        return render_unsupported();
    }
    let Some(v) = view.read().clone() else {
        return rsx! {
            div { class: "m-player-loading", p { class: "subtitle", "Loading\u{2026}" } }
        };
    };

    let elapsed_now = elapsed();
    let dur = if duration() > 0.0 {
        duration()
    } else {
        v.total_duration
    };
    let ch_idx = chapter_index_for_elapsed(&v.chapters, elapsed_now);

    render_player(PlayerProps {
        uuid: uuid.clone(),
        view: v,
        elapsed: elapsed_now,
        duration: dur,
        playing: playing(),
        rate: rate(),
        chapter_index: ch_idx,
        chapters_open,
        set_rate: EventHandler::new(move |r: f64| {
            rate.set(r);
            crate::audiobook_progress::save_rate(&uuid, r);
            interop::set_rate(r);
        }),
        playing_signal: playing,
    })
}

/// Bundle of props threaded into [`render_player`] so the signal-wiring
/// component body stays under the line cap.
struct PlayerProps {
    uuid: String,
    view: PlayerView,
    elapsed: f64,
    duration: f64,
    playing: bool,
    rate: f64,
    chapter_index: usize,
    chapters_open: Signal<bool>,
    set_rate: EventHandler<f64>,
    playing_signal: Signal<bool>,
}

/// Signals the async loader writes into. Grouped so [`load_player`] takes one
/// named argument rather than a wall of positional signals.
#[derive(Clone)]
struct LoadCtx {
    uuid: String,
    server_url: String,
    view: Signal<Option<PlayerView>>,
    error: Signal<Option<String>>,
    unsupported: Signal<bool>,
    duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    rate: Signal<f64>,
}

/// Async loader: fetch metadata + manifest, seed the view, then install the
/// audio control surface for direct-play books (or flag HLS as unsupported).
async fn load_player(ctx: LoadCtx) {
    let LoadCtx {
        uuid,
        server_url,
        mut view,
        mut error,
        mut unsupported,
        mut duration,
        mut elapsed,
        playing,
        rate,
    } = ctx;

    let book = match data::get_ebook(&server_url, &uuid).await {
        Ok(Some(b)) => b,
        Ok(None) => {
            error.set(Some("Audiobook not found.".into()));
            return;
        }
        Err(e) => {
            error.set(Some(e.to_string()));
            return;
        }
    };
    let file_id = book.book_files.first().map(|f| f.id);
    let manifest = match data::get_manifest(&server_url, &uuid, file_id).await {
        Ok(m) => m,
        Err(e) => {
            error.set(Some(e.to_string()));
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
            let eval = interop::install_direct_surface(&server_url, &parts, resume, *rate.peek());
            drain_audio_events(eval, uuid, server_url, elapsed, playing).await;
        }
        AudiobookManifest::Hls { .. } => {
            // hls.js isn't bundled on mobile; don't fake playback.
            view.set(Some(PlayerView::from_hls(&book)));
            unsupported.set(true);
        }
    }
}

/// Drain the JS→Rust audio event channel forever, updating the position /
/// playing signals and throttling position persistence to ~5s deltas.
async fn drain_audio_events(
    mut eval: dioxus::document::Eval,
    uuid: String,
    server_url: String,
    mut elapsed: Signal<f64>,
    mut playing: Signal<bool>,
) {
    let mut last_saved = 0.0_f64;
    loop {
        match eval.recv::<interop::AudioEvent>().await {
            Ok(interop::AudioEvent::Time { seconds }) => {
                elapsed.set(seconds);
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
            // Channel closed (surface torn down / navigation) — stop draining.
            Err(_) => return,
        }
    }
}

/// Reconcile the resume position: server-authoritative value wins, else the
/// locally cached position, else 0.
async fn resolve_resume(server_url: &str, uuid: &str) -> f64 {
    let server_pos = data::get_progress(server_url, uuid, ProgressFormat::Audio)
        .await
        .ok()
        .flatten()
        .and_then(|r| r.audio_position_seconds);
    server_pos
        .or_else(|| crate::audiobook_progress::load(uuid))
        .unwrap_or(0.0)
}

/// Persist the latest position both locally and to the server (fire-and-forget).
fn persist_position(uuid: &str, server_url: &str, seconds: f64) {
    crate::audiobook_progress::save(uuid, seconds);
    let uuid = uuid.to_string();
    let server_url = server_url.to_string();
    spawn(async move {
        let update = ProgressUpdate {
            book_uuid: uuid,
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(seconds),
        };
        let _ = data::save_progress(&server_url, update).await;
    });
}

fn render_error(msg: &str) -> Element {
    rsx! {
        div { class: "m-player-msg",
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        }
    }
}

fn render_unsupported() -> Element {
    rsx! {
        div { class: "m-player-msg",
            p { class: "subtitle",
                "This audiobook uses a streaming format not yet supported on mobile."
            }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        }
    }
}

/// Render the full player surface: cover hero, now-playing block, chapter
/// scrubber, transport row, secondary controls, and the chapters sheet.
fn render_player(p: PlayerProps) -> Element {
    let PlayerProps {
        uuid,
        view,
        elapsed,
        duration,
        playing,
        rate,
        chapter_index,
        mut chapters_open,
        set_rate,
        playing_signal,
    } = p;

    let server_url = use_server_url();
    let current = view.chapters.get(chapter_index);
    let chapter_no = chapter_index + 1;
    let chapter_count = view.chapters.len();
    let chapter_title = current.map(|c| c.title.clone()).unwrap_or_default();
    let chapter_start = current.map(|c| c.start_seconds).unwrap_or(0.0);
    let chapter_dur = current.map(|c| c.duration_seconds).unwrap_or(0.0);
    let within = (elapsed - chapter_start).max(0.0);
    let chapter_left = view::remaining_in_chapter(&view.chapters, chapter_index, elapsed);
    let remaining_book = (duration - elapsed).max(0.0);
    let scrub_max = if duration > 0.0 { duration } else { 1.0 };

    let accent_style = view
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    let rate_label = format!("{rate:.2}\u{00d7}");
    let has_chapters = !view.chapters.is_empty();

    // Transport handlers — all route through the JS control surface.
    let uuid_seek = uuid.clone();
    let su_seek = server_url.clone();
    let on_seek = move |evt: Event<FormData>| {
        if let Ok(secs) = evt.value().parse::<f64>() {
            interop::seek(secs);
            persist_position(&uuid_seek, &su_seek, secs);
        }
    };
    let mut playing_sig = playing_signal;
    let on_toggle = move |_| {
        interop::toggle();
        let now = *playing_sig.peek();
        playing_sig.set(!now);
    };
    let on_back = move |_| interop::skip(-30.0);
    let on_fwd = move |_| interop::skip(30.0);

    let chs_prev = view.chapters.clone();
    let on_prev = move |_: MouseEvent| {
        if let Some(t) = view::chapter_prev_seek(&chs_prev, elapsed, chapter_index) {
            interop::seek(t);
        }
    };
    let chs_next = view.chapters.clone();
    let on_next = move |_: MouseEvent| {
        if chapter_index + 1 < chs_next.len() {
            interop::seek(chs_next[chapter_index + 1].start_seconds);
        }
    };
    let set_rate_cycle = set_rate;
    let on_speed = move |_: MouseEvent| {
        let next = view::next_rate(rate);
        set_rate_cycle.call(next);
    };

    rsx! {
        div { class: "m-player", style: "{accent_style}", "data-testid": "mobile-player",
            div { class: "m-player-glow" }

            // Top bar
            div { class: "m-player-bar",
                Link { to: Route::BookDetail { uuid: uuid.clone() }, class: "m-icon-btn", "aria-label": "Back", "\u{2190}" }
            }

            // Cover hero
            div { class: "m-player-cover",
                Cover {
                    book: view.book.clone(),
                    src_override: cover_src(&view.book, &uuid, &server_url),
                    sizes: Some("220px".to_string()),
                }
            }

            // Now playing
            div { class: "m-player-now",
                div { class: "label m-player-eyebrow",
                    "Now playing \u{00b7} Chapter {chapter_no} of {chapter_count}"
                }
                h1 { class: "m-player-title", span { class: "m-em", "{view.title}" } }
                div { class: "m-player-by", "by {view.author}" }
                if has_chapters {
                    div { class: "m-player-chline", "Ch. {chapter_no} \u{00b7} {chapter_title}" }
                }
            }

            // Chapter scrubber
            div { class: "m-player-scrub",
                input {
                    class: "m-player-range",
                    r#type: "range",
                    "aria-label": "Seek",
                    "data-testid": "mobile-player-seek",
                    min: "0",
                    max: "{scrub_max}",
                    step: "1",
                    value: "{elapsed}",
                    oninput: on_seek,
                }
                div { class: "m-player-times mono",
                    span { "{format_hms(elapsed)}" }
                    if has_chapters {
                        span { class: "m-player-chtime",
                            "{format_ms(within)} / {format_ms(chapter_dur)} \u{00b7} {format_ms(chapter_left)} left"
                        }
                    }
                    span { "-{format_hms(remaining_book)}" }
                }
            }

            // Transport row
            div { class: "m-player-transport",
                button { class: "m-tp-ghost", r#type: "button", disabled: true, title: "Favorite (coming soon)", "aria-label": "Favorite", "\u{2661}" }
                button {
                    class: "m-tp-ch", r#type: "button", disabled: !has_chapters,
                    "aria-label": "Previous chapter", onclick: on_prev, "\u{25C1}|"
                }
                button { class: "m-tp-skip", r#type: "button", "data-testid": "mobile-skip-back", "aria-label": "Back 30 seconds", onclick: on_back, "-30" }
                button {
                    class: "m-tp-play", r#type: "button", "data-testid": "mobile-toggle",
                    "aria-label": if playing { "Pause" } else { "Play" }, onclick: on_toggle,
                    if playing {
                        span { class: "m-tp-pause", span {} span {} }
                    } else {
                        span { class: "m-tp-tri" }
                    }
                }
                button { class: "m-tp-skip", r#type: "button", "data-testid": "mobile-skip-forward", "aria-label": "Forward 30 seconds", onclick: on_fwd, "+30" }
                button {
                    class: "m-tp-ch", r#type: "button", disabled: !has_chapters,
                    "aria-label": "Next chapter", onclick: on_next, "|\u{25B7}"
                }
                button { class: "m-tp-rate", r#type: "button", "data-testid": "mobile-rate", "aria-label": "Playback speed", onclick: on_speed, "{rate_label}" }
            }

            // Secondary controls
            div { class: "m-player-secondary",
                button { class: "m-pill", r#type: "button", disabled: true, title: "Sleep timer (coming soon)", "Sleep" }
                button { class: "m-pill", r#type: "button", disabled: true, title: "Bookmark (coming soon)", "Bookmark" }
                button {
                    class: if chapters_open() { "m-pill on" } else { "m-pill" },
                    r#type: "button", "data-testid": "mobile-chapters-toggle",
                    onclick: move |_| { let c = *chapters_open.peek(); chapters_open.set(!c); },
                    "Chapters"
                }
                button { class: "m-pill", r#type: "button", disabled: true, title: "Cast (coming soon)", "Cast" }
            }

            if chapters_open() {
                ChaptersSheet {
                    chapters: view.chapters.clone(),
                    current_index: chapter_index,
                    elapsed,
                    total_label: view.total_label.clone(),
                    on_seek: EventHandler::new(move |secs: f64| { interop::seek(secs); chapters_open.set(false); }),
                    on_close: move |_| chapters_open.set(false),
                }
            }
        }
    }
}

/// Bottom-sheet chapter list. Current row is accent-highlighted with a mini
/// progress bar; done rows show a check, upcoming rows show their duration.
#[component]
fn ChaptersSheet(
    chapters: Vec<ChapterInfo>,
    current_index: usize,
    elapsed: f64,
    total_label: String,
    on_seek: EventHandler<f64>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let count = chapters.len();
    rsx! {
        div { class: "m-sheet-scrim", "data-testid": "mobile-chapters-sheet", onclick: move |e| on_close.call(e),
            div { class: "m-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "Chapters" }
                    span { class: "mono m-sheet-count", "{count} \u{00b7} {total_label}" }
                }
                div { class: "m-sheet-list",
                    for (i, ch) in chapters.iter().enumerate() {
                        {chapter_row(i, ch, current_index, elapsed, &on_seek)}
                    }
                }
            }
        }
    }
}

/// One chapter row in the sheet.
fn chapter_row(
    i: usize,
    ch: &ChapterInfo,
    current_index: usize,
    elapsed: f64,
    on_seek: &EventHandler<f64>,
) -> Element {
    let is_current = i == current_index;
    let is_done = i < current_index;
    let start = ch.start_seconds;
    let handler = *on_seek;
    let progress = if is_current && ch.duration_seconds > 0.0 {
        (((elapsed - start) / ch.duration_seconds).clamp(0.0, 1.0) * 100.0) as i64
    } else {
        0
    };
    rsx! {
        button {
            key: "{ch.ordinal}",
            class: if is_current { "m-ch-row current" } else { "m-ch-row" },
            r#type: "button",
            onclick: move |_| handler.call(start),
            span { class: "mono m-ch-num", "{ch.ordinal}" }
            div { class: "m-ch-body",
                div { class: "m-ch-title", span { class: "m-em", "{ch.title}" } }
                if is_current {
                    div { class: "pbar m-ch-pbar", i { style: "width:{progress}%" } }
                }
            }
            if is_current {
                span { class: "m-ch-trail m-ch-playing", "\u{25B6}" }
            } else if is_done {
                span { class: "m-ch-trail m-ch-done", "\u{2713}" }
            } else {
                span { class: "mono m-ch-trail m-ch-dur", "{format_ms(ch.duration_seconds)}" }
            }
        }
    }
}

/// Cover `src` for the hero — responsive thumbnail when a cover exists,
/// otherwise `None` so [`Cover`] renders its typographic plate.
fn cover_src(book: &EbookMetadata, uuid: &str, server_url: &str) -> Option<String> {
    if book.cover_url.is_some() {
        Some(crate::thumb_url(server_url, uuid, "lg"))
    } else {
        None
    }
}
