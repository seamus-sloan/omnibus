//! Post-load player chrome: top nav, hidden `<audio>`, status overlays,
//! and the two-column [`PlayerStage`]. Owns the per-action handlers
//! (back / toggle / skip / seek / rate) and overlay open/close state.
//! Rendered by the orchestrator after the `loading` / `error` /
//! `book.is_none()` gates.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::bookmarks::use_bookmarks;
use super::bookmarks_drawer::BookmarksDrawer;
use super::chapter_nav::{chapter_index_for_elapsed, chapter_next_target, chapter_prev_target};
use super::chapters_drawer::ChaptersDrawer;
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::sleep::{end_of_chapter_seconds, sleep_toolbar_label, use_sleep, SleepChoice};
use super::sleep_panel::{SleepPanel, SleepPanelState};
use super::speed_panel::SpeedPanel;
use super::stage::{
    PlaybackPosition, PlayerCallbacks, PlayerContent, PlayerStage, ToolbarState, TransportState,
};
use crate::Nav;

/// Scrub bar maximum — at least 1.0 so the range input is never empty.
pub(super) fn scrub_max(duration: f64) -> f64 {
    if duration > 0.0 {
        duration
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scrub_max_returns_duration_when_positive() {
        assert!((scrub_max(120.5) - 120.5).abs() < f64::EPSILON);
    }

    #[test]
    fn scrub_max_returns_one_when_duration_is_zero() {
        assert!((scrub_max(0.0) - 1.0).abs() < f64::EPSILON);
    }
}

/// Grouped playback signals (duration, elapsed, playing, rate, volume, hls_ready).
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PlaybackSignals {
    pub duration: Signal<f64>,
    pub elapsed: Signal<f64>,
    pub playing: Signal<bool>,
    pub rate: Signal<f64>,
    pub rate_error: Signal<Option<String>>,
    pub volume: Signal<f64>,
    pub hls_ready: Signal<bool>,
}

/// Render the ready-state player chrome and bind the transport handlers.
#[component]
pub(super) fn ReadyPlayer(
    book: EbookMetadata,
    uuid: String,
    signals: PlaybackSignals,
    playback_failed: Signal<bool>,
    chapters: Signal<Vec<ChapterInfo>>,
) -> Element {
    let elapsed = signals.elapsed;
    let rate = signals.rate;
    let rate_error = signals.rate_error;
    let hls_ready = signals.hls_ready;
    let speed_panel_open = use_signal(|| false);
    let sleep_panel_open = use_signal(|| false);
    let bookmarks_open = use_signal(|| false);
    let chapters_open = use_signal(|| false);

    // App-scoped sleep timer (provided at App root so it outlives this page)
    // + per-book bookmark state. Both hooks are declared unconditionally so
    // SSR/WASM hook order matches (rule 07); their web interop is gated
    // internally.
    let sleep = use_sleep();
    let bookmarks = use_bookmarks(uuid.clone());

    // Derive current chapter index from elapsed position.
    let current_chapter_index = use_memo(move || {
        let chs = chapters();
        let elapsed_now = elapsed();
        chapter_index_for_elapsed(&chs, elapsed_now)
    });

    let panes = OverlayPanes {
        speed_panel_open,
        sleep_panel_open,
        bookmarks_open,
        chapters_open,
    };

    // `seek` is shared by the stage's chapter list and the chapters/bookmarks
    // drawers, so it's built once here and handed to both children.
    let on_chapter_seek = move |secs: f64| super::helpers::seek_to(secs);

    // Sleep + bookmark actions are lifted here so `PlayerOverlays` can stay a
    // pure passthrough — `SleepController` isn't `PartialEq`, so it can't be a
    // component prop, but these `EventHandler`s can.
    let on_sleep_select = move |secs: i32| sleep.select_seconds(secs);
    let on_sleep_end_of_chapter = move |_: ()| {
        let chs_now = chapters.peek().clone();
        let idx = current_chapter_index();
        if let Some(secs) = end_of_chapter_seconds(&chs_now, idx, *elapsed.peek()) {
            sleep.select_end_of_chapter(secs);
        }
    };
    let on_sleep_toggle_fade = move |_: ()| sleep.toggle_fade();
    let on_bookmark_add = move |_: ()| {
        let chs_now = chapters.peek().clone();
        bookmarks.create(*elapsed.peek(), &chs_now);
    };

    let title = book.display_title();
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string());
    let ready = hls_ready();
    let failed = playback_failed();

    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    let chs = chapters();
    let ch_idx = current_chapter_index();

    // Reactive sleep-timer reads — re-render the toolbar label + panel live.
    let sleep_remaining = (sleep.remaining)();
    let sleep_active = matches!(sleep_remaining, Some(s) if s > 0);
    let sleep_display = SleepDisplay {
        remaining: sleep_remaining,
        choice: (sleep.choice)(),
        fade: (sleep.fade)(),
        has_chapters: !chs.is_empty(),
    };
    let bookmark_toast = (bookmarks.toast)();
    let current_user = crate::use_current_user_summary();
    let user_id = current_user().map(|user| user.id);

    rsx! {
        div { class: "lp-root", style: "{accent_style}",
            div { class: "lp-backdrop" }

            Nav {}

            if failed {
                FailedOverlay {}
            } else if !ready {
                PreparingOverlay {}
            }

            PlayerStageBinding {
                book: book.clone(),
                title,
                author,
                signals,
                panes,
                chapters,
                current_chapter_index,
                sleep_active: sleep_active || sleep_panel_open(),
                sleep_label: sleep_toolbar_label(sleep_remaining),
                on_chapter_seek: EventHandler::new(on_chapter_seek),
            }

            PlayerOverlays {
                panes,
                uuid: uuid.clone(),
                rate,
                rate_error,
                user_id,
                sleep_display,
                bookmarks,
                bookmark_toast,
                chapters: chs.clone(),
                current_chapter_index: ch_idx,
                elapsed: elapsed(),
                on_chapter_seek: EventHandler::new(on_chapter_seek),
                on_sleep_select: EventHandler::new(on_sleep_select),
                on_sleep_end_of_chapter: EventHandler::new(on_sleep_end_of_chapter),
                on_sleep_toggle_fade: EventHandler::new(on_sleep_toggle_fade),
                on_bookmark_add: EventHandler::new(on_bookmark_add),
            }
        }
    }
}

/// Assemble the transport + toolbar handlers and derived display values, then
/// render [`PlayerStage`]. Owns the play/skip/seek/rate/chapter callbacks and
/// the mutually-exclusive toolbar toggles.
#[component]
pub(super) fn PlayerStageBinding(
    book: EbookMetadata,
    title: String,
    author: String,
    signals: PlaybackSignals,
    panes: OverlayPanes,
    chapters: Signal<Vec<ChapterInfo>>,
    current_chapter_index: Memo<usize>,
    sleep_active: bool,
    sleep_label: String,
    on_chapter_seek: EventHandler<f64>,
) -> Element {
    let duration = signals.duration;
    let elapsed = signals.elapsed;
    let playing = signals.playing;
    let mut volume = signals.volume;
    let mut speed_panel_open = panes.speed_panel_open;
    let mut sleep_panel_open = panes.sleep_panel_open;
    let mut bookmarks_open = panes.bookmarks_open;
    let mut chapters_open = panes.chapters_open;

    let on_toggle = super::helpers::on_toggle_playback();
    let on_skip_back = super::helpers::on_skip_back_30();
    let on_skip_forward = super::helpers::on_skip_forward_30();
    let on_volume = move |v: f64| super::helpers::apply_volume(&mut volume, v);
    let on_rate = move |_: MouseEvent| {
        let cur = *speed_panel_open.peek();
        speed_panel_open.set(!cur);
        if !cur {
            sleep_panel_open.set(false);
            bookmarks_open.set(false);
            chapters_open.set(false);
        }
    };
    let on_chapter_prev = move |_: MouseEvent| {
        let chs = chapters();
        let idx = current_chapter_index();
        if let Some(target) = chapter_prev_target(&chs, elapsed(), idx) {
            super::helpers::seek_to(target);
        }
    };
    let on_chapter_next = move |_: MouseEvent| {
        let chs = chapters();
        let idx = current_chapter_index();
        if let Some(target) = chapter_next_target(&chs, idx) {
            super::helpers::seek_to(target);
        }
    };

    let dur = duration();
    let elapsed_now = elapsed();
    let chs = chapters();

    rsx! {
        PlayerStage {
            content: PlayerContent {
                book: book.clone(),
                title,
                author,
                chapters: chs.clone(),
            },
            position: PlaybackPosition {
                elapsed: elapsed_now,
                duration: dur,
                remaining: (dur - elapsed_now).max(0.0),
                scrub_max: scrub_max(dur),
                current_chapter_index: current_chapter_index(),
            },
            transport: TransportState {
                play_label: if playing() { "Pause" } else { "Play" }.to_string(),
                playing: playing(),
                rate_label: format!("{:.2}\u{00d7}", (signals.rate)()),
                rate_active: speed_panel_open(),
                has_chapters: !chs.is_empty(),
                volume: volume(),
            },
            callbacks: PlayerCallbacks {
                on_toggle: EventHandler::new(on_toggle),
                on_skip_back: EventHandler::new(on_skip_back),
                on_skip_forward: EventHandler::new(on_skip_forward),
                on_rate: EventHandler::new(on_rate),
                on_chapter_prev: EventHandler::new(on_chapter_prev),
                on_chapter_next: EventHandler::new(on_chapter_next),
                on_chapter_seek,
                on_volume: EventHandler::new(on_volume),
            },
            toolbar: ToolbarState {
                sleep_active,
                sleep_label,
                bookmarks_active: bookmarks_open(),
                chapters_active: chapters_open(),
                on_sleep: EventHandler::new(move |_| {
                    let cur = *sleep_panel_open.peek();
                    sleep_panel_open.set(!cur);
                    if !cur {
                        speed_panel_open.set(false);
                        bookmarks_open.set(false);
                        chapters_open.set(false);
                    }
                }),
                on_bookmark: EventHandler::new(move |_| {
                    let cur = *bookmarks_open.peek();
                    bookmarks_open.set(!cur);
                    if !cur {
                        speed_panel_open.set(false);
                        sleep_panel_open.set(false);
                        chapters_open.set(false);
                    }
                }),
                on_chapters: EventHandler::new(move |_| {
                    let cur = *chapters_open.peek();
                    chapters_open.set(!cur);
                    if !cur {
                        speed_panel_open.set(false);
                        sleep_panel_open.set(false);
                        bookmarks_open.set(false);
                    }
                }),
            },
        }
    }
}

/// Open/closed state for the four toolbar-driven panes.
#[derive(Copy, Clone, PartialEq)]
pub(super) struct OverlayPanes {
    pub speed_panel_open: Signal<bool>,
    pub sleep_panel_open: Signal<bool>,
    pub bookmarks_open: Signal<bool>,
    pub chapters_open: Signal<bool>,
}

/// Reactive sleep-timer values rendered by the sleep panel.
#[derive(Copy, Clone, PartialEq)]
pub(super) struct SleepDisplay {
    pub remaining: Option<i32>,
    pub choice: SleepChoice,
    pub fade: bool,
    pub has_chapters: bool,
}

/// The four toolbar-toggled surfaces plus the bookmark-saved toast. Each
/// renders only when its backing open-signal (or toast option) is set. Sleep
/// mutations arrive as `EventHandler`s so the non-`PartialEq` `SleepController`
/// stays out of props.
#[component]
pub(super) fn PlayerOverlays(
    panes: OverlayPanes,
    uuid: String,
    rate: Signal<f64>,
    rate_error: Signal<Option<String>>,
    user_id: Option<i64>,
    sleep_display: SleepDisplay,
    bookmarks: super::bookmarks::BookmarksController,
    bookmark_toast: Option<super::bookmarks::BookmarkToast>,
    chapters: Vec<ChapterInfo>,
    current_chapter_index: usize,
    elapsed: f64,
    on_chapter_seek: EventHandler<f64>,
    on_sleep_select: EventHandler<i32>,
    on_sleep_end_of_chapter: EventHandler<()>,
    on_sleep_toggle_fade: EventHandler<()>,
    on_bookmark_add: EventHandler<()>,
) -> Element {
    let mut speed_panel_open = panes.speed_panel_open;
    let mut sleep_panel_open = panes.sleep_panel_open;
    let mut bookmarks_open = panes.bookmarks_open;
    let mut chapters_open = panes.chapters_open;
    rsx! {
        if speed_panel_open() {
            SpeedPanel {
                rate,
                rate_error,
                user_id,
                uuid: uuid.clone(),
                on_close: move |_| speed_panel_open.set(false),
            }
        }
        if sleep_panel_open() {
            SleepPanel {
                state: SleepPanelState {
                    remaining: sleep_display.remaining,
                    choice: sleep_display.choice,
                    fade: sleep_display.fade,
                    has_chapters: sleep_display.has_chapters,
                    on_select: EventHandler::new(move |secs: i32| on_sleep_select.call(secs)),
                    on_end_of_chapter: EventHandler::new(move |_| on_sleep_end_of_chapter.call(())),
                    on_toggle_fade: EventHandler::new(move |_| on_sleep_toggle_fade.call(())),
                },
                on_close: move |_| sleep_panel_open.set(false),
            }
        }
        if bookmarks_open() {
            BookmarksDrawer {
                controller: bookmarks,
                chapters: chapters.clone(),
                on_seek: on_chapter_seek,
                on_add: move |_| on_bookmark_add.call(()),
                on_close: move |_| bookmarks_open.set(false),
            }
        }

        if let Some(toast) = bookmark_toast {
            div { class: "lp-toast", "data-testid": "bookmark-toast",
                span { class: "lp-toast-icon", "\u{2691}" }
                span { class: "lp-toast-text", "Bookmark saved" }
                span { class: "lp-toast-meta", "{toast.time_label} \u{00b7} {toast.chapter_label}" }
            }
        }
        if chapters_open() {
            ChaptersDrawer {
                chapters: chapters.clone(),
                current_chapter_index,
                elapsed,
                on_seek: on_chapter_seek,
                on_close: move |_| chapters_open.set(false),
            }
        }
        if let Some(message) = rate_error() {
            div {
                class: "lp-toast",
                role: "alert",
                "data-testid": "playback-rate-error",
                span { class: "lp-toast-text", "{message}" }
            }
        }
    }
}
