//! Post-load player chrome: top nav, hidden `<audio>`, status overlays,
//! and the two-column [`PlayerStage`]. Owns the per-action handlers
//! (back / toggle / skip / seek / rate) and overlay open/close state.
//! Rendered by the orchestrator after the `loading` / `error` /
//! `book.is_none()` gates.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::bookmarks::{use_bookmarks, BookmarksController};
use super::bookmarks_drawer::BookmarksDrawer;
use super::chapter_nav::{chapter_index_for_elapsed, chapter_next_target, chapter_prev_target};
use super::chapters_drawer::ChaptersDrawer;
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::sleep::{end_of_chapter_seconds, sleep_toolbar_label, use_sleep, SleepController};
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

/// Build the sleep-panel state: reactive countdown/choice/fade reads plus
/// the select/end-of-chapter/toggle-fade handlers. Split out of
/// [`ReadyPlayer`] so the sleep-controller wiring doesn't sit inline in the
/// component body.
fn build_sleep_panel_state(
    sleep: SleepController,
    chapters: Signal<Vec<ChapterInfo>>,
    elapsed: Signal<f64>,
    rate: Signal<f64>,
    current_chapter_index: Memo<usize>,
    has_chapters: bool,
) -> (SleepPanelState, Option<i32>, bool) {
    let on_sleep_select = move |secs: i32| sleep.select_seconds(secs);
    let on_sleep_end_of_chapter = move |_: ()| {
        let chs_now = chapters.peek().clone();
        let idx = current_chapter_index();
        if let Some(secs) = end_of_chapter_seconds(&chs_now, idx, *elapsed.peek(), *rate.peek()) {
            sleep.select_end_of_chapter(secs);
        }
    };
    let on_sleep_toggle_fade = move |_: ()| sleep.toggle_fade();
    // Read once and hand back alongside the state so callers (the toolbar
    // label/active flag) don't re-read the signal.
    let remaining = (sleep.remaining)();
    let active = matches!(remaining, Some(s) if s > 0);
    let state = SleepPanelState {
        remaining,
        choice: (sleep.choice)(),
        fade: (sleep.fade)(),
        has_chapters,
        on_select: EventHandler::new(on_sleep_select),
        on_end_of_chapter: EventHandler::new(on_sleep_end_of_chapter),
        on_toggle_fade: EventHandler::new(on_sleep_toggle_fade),
    };
    (state, remaining, active)
}

/// Display title, author byline, and accent CSS variable derived from
/// `book`. Split out of [`ReadyPlayer`] to shrink its derived-state prelude.
fn book_display_fields(book: &EbookMetadata) -> (String, String, String) {
    let title = book.display_title();
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string());
    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    (title, author, accent_style)
}

/// Bookmark "add" handler: creates one at the current elapsed position from
/// a fresh chapter-list snapshot. Split out of [`ReadyPlayer`] alongside
/// [`build_sleep_panel_state`].
fn build_bookmark_add_handler(
    bookmarks: BookmarksController,
    chapters: Signal<Vec<ChapterInfo>>,
    elapsed: Signal<f64>,
) -> impl FnMut(()) + 'static {
    move |_: ()| {
        let chs_now = chapters.peek().clone();
        bookmarks.create(*elapsed.peek(), &chs_now);
    }
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
    let panes = OverlayPanes {
        speed_panel_open: use_signal(|| false),
        sleep_panel_open: use_signal(|| false),
        bookmarks_open: use_signal(|| false),
        chapters_open: use_signal(|| false),
    };

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
    // `seek` is shared by the stage's chapter list and the chapters/bookmarks
    // drawers, so it's built once here and handed to both children.
    let on_chapter_seek = move |secs: f64| super::helpers::seek_to(secs);

    // Bookmark action is lifted here so `PlayerOverlays` can stay a pure
    // passthrough — `EventHandler`s can be component props, unlike the raw
    // controller.
    let on_bookmark_add = build_bookmark_add_handler(bookmarks, chapters, elapsed);
    let (title, author, accent_style) = book_display_fields(&book);
    let ready = hls_ready();
    let failed = playback_failed();
    let chs = chapters();
    let ch_idx = current_chapter_index();
    let (sleep_state, sleep_remaining, sleep_active) = build_sleep_panel_state(
        sleep,
        chapters,
        elapsed,
        signals.rate,
        current_chapter_index,
        !chs.is_empty(),
    );
    let bookmark_toast = (bookmarks.toast)();
    let user_id = crate::use_current_user_summary()().map(|user| user.id);

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
                book_meta: BookMeta { book: book.clone(), title, author },
                signals,
                panes,
                chapter_state: ChapterSignals {
                    chapters,
                    current_chapter_index,
                    on_chapter_seek: EventHandler::new(on_chapter_seek),
                },
                sleep_toolbar: SleepToolbarState {
                    active: sleep_active || (panes.sleep_panel_open)(),
                    label: sleep_toolbar_label(sleep_remaining),
                },
            }

            PlayerOverlays {
                panes,
                speed: SpeedPanelData { rate, rate_error, user_id, uuid: uuid.clone() },
                sleep_state,
                bookmark: BookmarkPanelData {
                    controller: bookmarks,
                    toast: bookmark_toast,
                    on_add: EventHandler::new(on_bookmark_add),
                },
                chapter_nav: ChapterNavData {
                    chapters: chs.clone(),
                    current_chapter_index: ch_idx,
                    elapsed: elapsed(),
                    on_seek: EventHandler::new(on_chapter_seek),
                },
            }
        }
    }
}

/// Book identity fields for [`PlayerStageBinding`]'s content pane. Grouped
/// so the binding component stays under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct BookMeta {
    pub book: EbookMetadata,
    pub title: String,
    pub author: String,
}

/// Chapter list, derived current-chapter index, and the shared seek handler.
/// Grouped so [`PlayerStageBinding`] stays under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct ChapterSignals {
    pub chapters: Signal<Vec<ChapterInfo>>,
    pub current_chapter_index: Memo<usize>,
    pub on_chapter_seek: EventHandler<f64>,
}

/// Sleep-toolbar badge state: lit or not, and its countdown/preset label.
/// Grouped so [`PlayerStageBinding`] stays under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct SleepToolbarState {
    pub active: bool,
    pub label: String,
}

/// Build a handler that toggles `target` and, when it's opening, closes the
/// other three toolbar panes. The four toolbar toggles (speed/sleep/
/// bookmarks/chapters) are mutually exclusive — only one pane is open at a
/// time.
fn toggle_pane_handler(
    mut target: Signal<bool>,
    others: [Signal<bool>; 3],
) -> impl FnMut(MouseEvent) + 'static {
    move |_: MouseEvent| {
        let cur = *target.peek();
        target.set(!cur);
        if !cur {
            for mut other in others {
                other.set(false);
            }
        }
    }
}

/// Build the previous/next chapter-step handlers shared by the transport row.
fn build_chapter_step_handlers(
    chapters: Signal<Vec<ChapterInfo>>,
    current_chapter_index: Memo<usize>,
    elapsed: Signal<f64>,
) -> (
    impl FnMut(MouseEvent) + 'static,
    impl FnMut(MouseEvent) + 'static,
) {
    let on_prev = move |_: MouseEvent| {
        let chs = chapters();
        let idx = current_chapter_index();
        if let Some(target) = chapter_prev_target(&chs, elapsed(), idx) {
            super::helpers::seek_to(target);
        }
    };
    let on_next = move |_: MouseEvent| {
        let chs = chapters();
        let idx = current_chapter_index();
        if let Some(target) = chapter_next_target(&chs, idx) {
            super::helpers::seek_to(target);
        }
    };
    (on_prev, on_next)
}

/// Assemble the transport + toolbar handlers and derived display values, then
/// render [`PlayerStage`]. Owns the play/skip/seek/rate/chapter callbacks and
/// the mutually-exclusive toolbar toggles.
#[component]
pub(super) fn PlayerStageBinding(
    book_meta: BookMeta,
    signals: PlaybackSignals,
    panes: OverlayPanes,
    chapter_state: ChapterSignals,
    sleep_toolbar: SleepToolbarState,
) -> Element {
    let BookMeta {
        book,
        title,
        author,
    } = book_meta;
    let ChapterSignals {
        chapters,
        current_chapter_index,
        on_chapter_seek,
    } = chapter_state;
    let SleepToolbarState {
        active: sleep_active,
        label: sleep_label,
    } = sleep_toolbar;
    let duration = signals.duration;
    let elapsed = signals.elapsed;
    let playing = signals.playing;
    let mut volume = signals.volume;
    let speed_panel_open = panes.speed_panel_open;
    let sleep_panel_open = panes.sleep_panel_open;
    let bookmarks_open = panes.bookmarks_open;
    let chapters_open = panes.chapters_open;
    let on_toggle = super::helpers::on_toggle_playback();
    let on_skip_back = super::helpers::on_skip_back_30();
    let on_skip_forward = super::helpers::on_skip_forward_30();
    let on_volume = move |v: f64| super::helpers::apply_volume(&mut volume, v);
    let on_rate = toggle_pane_handler(
        speed_panel_open,
        [sleep_panel_open, bookmarks_open, chapters_open],
    );
    let (on_chapter_prev, on_chapter_next) =
        build_chapter_step_handlers(chapters, current_chapter_index, elapsed);
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
                rate: (signals.rate)(),
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
                on_sleep: EventHandler::new(toggle_pane_handler(
                    sleep_panel_open,
                    [speed_panel_open, bookmarks_open, chapters_open],
                )),
                on_bookmark: EventHandler::new(toggle_pane_handler(
                    bookmarks_open,
                    [speed_panel_open, sleep_panel_open, chapters_open],
                )),
                on_chapters: EventHandler::new(toggle_pane_handler(
                    chapters_open,
                    [speed_panel_open, sleep_panel_open, bookmarks_open],
                )),
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

/// Rate + user context needed to open [`SpeedPanel`] — mirrors that panel's
/// own prop list (minus `on_close`). Grouped so [`PlayerOverlays`] stays
/// under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct SpeedPanelData {
    pub rate: Signal<f64>,
    pub rate_error: Signal<Option<String>>,
    pub user_id: Option<i64>,
    pub uuid: String,
}

/// Bookmark controller, pending save-toast, and the "add bookmark" handler.
/// Grouped so [`PlayerOverlays`] stays under the prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct BookmarkPanelData {
    pub controller: super::bookmarks::BookmarksController,
    pub toast: Option<super::bookmarks::BookmarkToast>,
    pub on_add: EventHandler<()>,
}

/// Chapter list, current position, and the seek handler shared by the
/// bookmarks/chapters drawers. Grouped so [`PlayerOverlays`] stays under the
/// prop cap.
#[derive(Clone, PartialEq)]
pub(super) struct ChapterNavData {
    pub chapters: Vec<ChapterInfo>,
    pub current_chapter_index: usize,
    pub elapsed: f64,
    pub on_seek: EventHandler<f64>,
}

/// The four toolbar-toggled surfaces plus the bookmark-saved toast. Each
/// renders only when its backing open-signal (or toast option) is set. Sleep
/// mutations arrive inside `sleep_state` as `EventHandler`s so the
/// non-`PartialEq` `SleepController` stays out of props.
#[component]
pub(super) fn PlayerOverlays(
    panes: OverlayPanes,
    speed: SpeedPanelData,
    sleep_state: SleepPanelState,
    bookmark: BookmarkPanelData,
    chapter_nav: ChapterNavData,
) -> Element {
    let mut speed_panel_open = panes.speed_panel_open;
    let mut sleep_panel_open = panes.sleep_panel_open;
    let mut bookmarks_open = panes.bookmarks_open;
    let mut chapters_open = panes.chapters_open;
    let SpeedPanelData {
        rate,
        rate_error,
        user_id,
        uuid,
    } = speed;
    let BookmarkPanelData {
        controller: bookmarks,
        toast: bookmark_toast,
        on_add: on_bookmark_add,
    } = bookmark;
    let ChapterNavData {
        chapters,
        current_chapter_index,
        elapsed,
        on_seek: on_chapter_seek,
    } = chapter_nav;
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
                state: sleep_state,
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
