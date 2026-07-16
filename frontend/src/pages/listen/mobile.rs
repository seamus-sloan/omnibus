//! Mobile audiobook player — the single-column phone adaptation of the
//! desktop two-column "Now playing" surface.
//!
//! The page is a view over the app-wide [`state::MobilePlayback`] context:
//! on mount it retargets `ctx.uuid` and the app-root [`host::MobileAudioHost`]
//! does the manifest fetch, drives the HTML `<audio>` element (via
//! `dioxus::document::eval`, since mobile is a wry WebView), and persists
//! position + rate through [`crate::audiobook_progress`] — so playback and
//! position tracking survive navigating away (the mini-player takes over).
//! HLS manifests render an unsupported state rather than faking playback.

#![cfg(feature = "mobile")]

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{EbookMetadata, ProgressFormat, ProgressUpdate};

use crate::components::atrium::Cover;
use crate::contexts::use_server_url;
use crate::data;
use crate::Route;

mod bookmarks_sheet;
mod host;
mod interop;
mod mini;
mod sheets;
mod state;
mod view;

use bookmarks_sheet::{use_mobile_bookmarks, BookmarksSheet, MobileBookmarks};
use sheets::{snap_rate, ChaptersSheet, SleepSheet, SpeedSheet};
use state::{sleep_pill_label, use_mobile_playback, SleepState};
use view::{chapter_index_for_elapsed, format_hms, format_ms, remaining_at_rate, PlayerView};

pub use host::MobileAudioHost;
pub use mini::MobileMiniPlayer;
pub use state::MobilePlayback;

/// Which bottom sheet is open, if any. One surface at a time — opening a
/// sheet replaces the previous one, matching the design's modal sheets.
#[derive(Clone, Copy, PartialEq, Eq)]
enum OpenSheet {
    None,
    Chapters,
    Speed,
    Sleep,
    Bookmarks,
}

/// Renders the mobile audiobook player for `uuid`. Retargets the app-wide
/// playback context and renders its state; the heavy lifting (fetch, audio
/// surface, event drain) lives in the app-root [`host::MobileAudioHost`].
#[component]
pub fn MobilePlayer(uuid: String) -> Element {
    let server_url = use_server_url();
    let ctx = use_mobile_playback();
    let sheet = use_signal(|| OpenSheet::None);
    let bookmarks = use_mobile_bookmarks(uuid.clone(), server_url.clone());
    let nav = use_navigator();

    // Back affordance: unwind the history stack rather than pushing a fresh
    // `BookDetail`. The player is reached *from* the detail page, so a push
    // stacks a second Detail entry and traps "back" bouncing between the two;
    // `go_back` returns to wherever the user actually came from. Falls back to
    // a Detail push only on a cold deep-link into `/listen/:uuid` (no history).
    let back_uuid = uuid.clone();
    let on_back = EventHandler::new(move |_: MouseEvent| {
        if nav.can_go_back() {
            nav.go_back();
        } else {
            nav.push(Route::BookDetail {
                uuid: back_uuid.clone(),
            });
        }
    });

    // Point the app-wide player at this route's book — only when it differs,
    // so re-entering the currently-playing book (e.g. from the mini-player)
    // is seamless instead of restarting the surface.
    let route_uuid = uuid.clone();
    use_effect(use_reactive!(|route_uuid| {
        let mut uuid_sig = ctx.uuid;
        if uuid_sig.peek().as_deref() != Some(route_uuid.as_str()) {
            uuid_sig.set(Some(route_uuid.clone()));
        }
    }));

    // Re-measure the title marquee whenever the displayed title (re)appears —
    // covers the loading→loaded transition and any book switch. A hook, so it
    // must run unconditionally ahead of the early returns below.
    let view_now = (ctx.view)();
    let marquee_title = view_now.as_ref().map(|v| v.title.clone());
    use_effect(use_reactive!(|marquee_title| {
        if marquee_title.is_some() {
            interop::refresh_title_marquee();
        }
    }));

    if let Some(msg) = (ctx.error)() {
        return render_error(&msg);
    }
    let Some(v) = view_now else {
        return rsx! {
            div { class: "m-player-loading", p { class: "subtitle", "Loading\u{2026}" } }
        };
    };
    if (ctx.unsupported)() {
        return render_unsupported(&v, &uuid, &server_url, on_back);
    }

    let elapsed_now = (ctx.elapsed)();
    let dur = if (ctx.duration)() > 0.0 {
        (ctx.duration)()
    } else {
        v.total_duration
    };
    let ch_idx = chapter_index_for_elapsed(&v.chapters, elapsed_now);

    render_player(PlayerProps {
        uuid: uuid.clone(),
        view: v,
        elapsed: elapsed_now,
        duration: dur,
        playing: (ctx.playing)(),
        rate: (ctx.rate)(),
        sleep: (ctx.sleep)(),
        chapter_index: ch_idx,
        sheet,
        bookmarks,
        ctx,
        on_nav_back: on_back,
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
    sleep: SleepState,
    chapter_index: usize,
    sheet: Signal<OpenSheet>,
    bookmarks: MobileBookmarks,
    ctx: MobilePlayback,
    /// Top-bar back affordance (distinct from the `-30s` transport skip).
    on_nav_back: EventHandler<MouseEvent>,
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

/// Marquee-ready title markup: `.m-player-title` is the fixed-width clipping
/// container; `.m-player-title-track` holds two identical copies of the
/// title (CSS hides the second by default). [`interop::refresh_title_marquee`]
/// measures only the first copy and toggles `.is-overflowing` (which reveals
/// the second copy and starts the loop) only when the title is wider than
/// its container, so a short title stays static and never shows duplicated
/// text before the measurement JS runs.
fn player_title(title: &str) -> Element {
    rsx! {
        h1 { class: "m-player-title",
            span { class: "m-player-title-track",
                span { class: "m-em", "{title}" }
                span { class: "m-em", "aria-hidden": "true", "{title}" }
            }
        }
    }
}

fn render_error(msg: &str) -> Element {
    rsx! {
        div { class: "m-player-msg",
            p { role: "alert", class: "subtitle", "{msg}" }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        }
    }
}

/// Streaming (HLS) books can't play on mobile yet, but we still show the cover
/// hero + title/author so the screen isn't a bare error — only the transport is
/// withheld behind the unsupported message.
fn render_unsupported(
    view: &PlayerView,
    uuid: &str,
    server_url: &str,
    on_back: EventHandler<MouseEvent>,
) -> Element {
    let accent_style = view
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    rsx! {
        div { class: "m-player", style: "{accent_style}",
            div { class: "m-player-glow" }
            div { class: "m-player-bar",
                button { r#type: "button", class: "m-icon-btn", "aria-label": "Back", onclick: move |e| on_back.call(e), "\u{2190}" }
            }
            div { class: "m-player-cover",
                Cover {
                    book: view.book.clone(),
                    src_override: cover_src(&view.book, uuid, server_url),
                    sizes: Some("220px".to_string()),
                }
            }
            div { class: "m-player-now",
                {player_title(&view.title)}
                div { class: "m-player-by", "by {view.author}" }
            }
            div { class: "m-player-msg",
                p { class: "subtitle",
                    "This audiobook uses a streaming format not yet supported on mobile."
                }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}

/// Render the full player surface: cover hero, now-playing block, chapter
/// scrubber, transport row, secondary controls, toast, and the bottom sheets.
fn render_player(p: PlayerProps) -> Element {
    let PlayerProps {
        uuid,
        view,
        elapsed,
        duration,
        playing,
        rate,
        sleep,
        chapter_index,
        mut sheet,
        bookmarks,
        ctx,
        on_nav_back,
    } = p;

    let server_url = use_server_url();
    let current = view.chapters.get(chapter_index);
    let chapter_no = chapter_index + 1;
    let chapter_count = view.chapters.len();
    let chapter_title = current.map(|c| c.title.clone()).unwrap_or_default();
    let chapter_start = current.map(|c| c.start_seconds).unwrap_or(0.0);
    let chapter_dur = current.map(|c| c.duration_seconds).unwrap_or(0.0);
    let within = (elapsed - chapter_start).max(0.0);
    let chapter_left = remaining_at_rate(
        view::remaining_in_chapter(&view.chapters, chapter_index, elapsed),
        rate,
    );
    let remaining_book = remaining_at_rate((duration - elapsed).max(0.0), rate);
    let scrub_max = if duration > 0.0 { duration } else { 1.0 };

    let accent_style = view
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    let rate_label = format!("{rate:.2}\u{00d7}");
    let sleep_label = sleep_pill_label(sleep, elapsed);
    let sleep_armed = sleep != SleepState::Off;
    let has_chapters = !view.chapters.is_empty();
    let toast = (bookmarks.toast)();

    // Transport handlers — all route through the JS control surface.
    // Seek live on every input, but only persist on release (`onchange`) so
    // dragging the scrubber doesn't spam local writes + server POSTs.
    let on_seek_input = move |evt: Event<FormData>| {
        if let Ok(secs) = evt.value().parse::<f64>() {
            interop::seek(secs);
        }
    };
    let uuid_seek = uuid.clone();
    let su_seek = server_url.clone();
    let on_seek_commit = move |evt: Event<FormData>| {
        if let Ok(secs) = evt.value().parse::<f64>() {
            persist_position(&uuid_seek, &su_seek, secs);
        }
    };
    let mut playing_sig = ctx.playing;
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

    let chs_mark = view.chapters.clone();
    let on_bookmark = move |_: MouseEvent| {
        bookmarks.create(elapsed, &chs_mark);
        sheet.set(OpenSheet::Bookmarks);
    };

    let sheet_props = SheetProps {
        uuid: uuid.clone(),
        view: view.clone(),
        elapsed,
        rate,
        sleep,
        chapter_index,
        sheet,
        bookmarks,
        ctx,
        server_url: server_url.clone(),
    };

    rsx! {
        div { class: "m-player", style: "{accent_style}", "data-testid": "mobile-player",
            div { class: "m-player-glow" }

            // Top bar
            div { class: "m-player-bar",
                button { r#type: "button", class: "m-icon-btn", "aria-label": "Back", onclick: move |e| on_nav_back.call(e), "\u{2190}" }
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
                {player_title(&view.title)}
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
                    oninput: on_seek_input,
                    onchange: on_seek_commit,
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

            // Transport row — five primary controls. Keeping it to five (vs
            // folding in favorite/speed) is what keeps the 72px play button
            // circular on a phone; speed lives in the secondary row below.
            div { class: "m-player-transport",
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
            }

            // Secondary controls — each pill opens its bottom sheet.
            div { class: "m-player-secondary",
                button {
                    class: if sheet() == OpenSheet::Speed { "m-pill on" } else { "m-pill" },
                    r#type: "button", "data-testid": "mobile-rate", "aria-label": "Playback speed",
                    onclick: move |_| sheet.set(OpenSheet::Speed),
                    "{rate_label}"
                }
                button {
                    class: if sheet() == OpenSheet::Sleep || sleep_armed { "m-pill on" } else { "m-pill" },
                    r#type: "button", "data-testid": "mobile-sleep",
                    onclick: move |_| sheet.set(OpenSheet::Sleep),
                    "{sleep_label}"
                }
                button {
                    class: "m-pill", r#type: "button", "data-testid": "mobile-bookmark",
                    onclick: on_bookmark,
                    "Bookmark"
                }
                button {
                    class: if sheet() == OpenSheet::Chapters { "m-pill on" } else { "m-pill" },
                    r#type: "button", "data-testid": "mobile-chapters-toggle",
                    onclick: move |_| sheet.set(OpenSheet::Chapters),
                    "Chapters"
                }
            }

            // "Bookmark saved" confirmation toast.
            if let Some(label) = toast {
                div { class: "m-toast", role: "status",
                    span { class: "m-toast-flag", "\u{2691}" }
                    span { class: "m-toast-text", "Bookmark saved" }
                    span { class: "mono m-toast-meta", "{label}" }
                }
            }
            if let Some(message) = (p.ctx.rate_error)() {
                div { class: "m-toast", role: "alert",
                    span { class: "m-toast-text", "{message}" }
                }
            }

            {render_sheets(&sheet_props)}
        }
    }
}

/// Everything the sheet layer needs, bundled so [`render_player`] stays a
/// transport-focused function.
struct SheetProps {
    uuid: String,
    view: PlayerView,
    elapsed: f64,
    rate: f64,
    sleep: SleepState,
    chapter_index: usize,
    sheet: Signal<OpenSheet>,
    bookmarks: MobileBookmarks,
    ctx: MobilePlayback,
    server_url: String,
}

/// Mount whichever bottom sheet is open.
fn render_sheets(p: &SheetProps) -> Element {
    let mut sheet = p.sheet;
    let close = move |_: MouseEvent| sheet.set(OpenSheet::None);

    match (p.sheet)() {
        OpenSheet::None => rsx! {},
        OpenSheet::Chapters => rsx! {
            ChaptersSheet {
                chapters: p.view.chapters.clone(),
                current_index: p.chapter_index,
                elapsed: p.elapsed,
                total_label: p.view.total_label.clone(),
                on_seek: EventHandler::new(move |secs: f64| {
                    interop::seek(secs);
                    sheet.set(OpenSheet::None);
                }),
                on_close: close,
            }
        },
        OpenSheet::Speed => {
            let uuid = p.uuid.clone();
            let server_url = p.server_url.clone();
            let mut rate_sig = p.ctx.rate;
            let rate_error = p.ctx.rate_error;
            let user_id = *p.ctx.user_id.peek();
            rsx! {
                SpeedSheet {
                    rate: p.rate,
                    on_set: EventHandler::new(move |r: f64| {
                        let snapped = snap_rate(r);
                        rate_sig.set(snapped);
                        if let Some(user_id) = user_id {
                            crate::audiobook_progress::save_rate(user_id, &uuid, snapped);
                        }
                        interop::set_rate(snapped);
                        let uuid = uuid.clone();
                        let server_url = server_url.clone();
                        spawn(async move {
                            let mut rate_error = rate_error;
                            let update = omnibus_shared::AudiobookPlaybackRateUpdate {
                                playback_rate: snapped,
                            };
                            match data::set_playback_rate(&server_url, &uuid, update).await {
                                Ok(_) => rate_error.set(None),
                                Err(error) => rate_error.set(Some(format!(
                                    "Could not save playback speed: {error}"
                                ))),
                            }
                        });
                    }),
                    on_close: close,
                }
            }
        }
        OpenSheet::Sleep => {
            let chapter_end = p
                .view
                .chapters
                .get(p.chapter_index)
                .map(|c| c.start_seconds + c.duration_seconds);
            let mut sleep_sig = p.ctx.sleep;
            rsx! {
                SleepSheet {
                    sleep: p.sleep,
                    elapsed: p.elapsed,
                    chapter_end,
                    chapter_no: p.chapter_index + 1,
                    on_set: EventHandler::new(move |s: SleepState| sleep_sig.set(s)),
                    on_close: close,
                }
            }
        }
        OpenSheet::Bookmarks => {
            let uuid = p.uuid.clone();
            let server_url = p.server_url.clone();
            rsx! {
                BookmarksSheet {
                    bookmarks: p.bookmarks,
                    chapters: p.view.chapters.clone(),
                    on_seek: EventHandler::new(move |secs: f64| {
                        interop::seek(secs);
                        persist_position(&uuid, &server_url, secs);
                        sheet.set(OpenSheet::None);
                    }),
                    on_close: close,
                }
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
