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
use super::chapters_drawer::ChaptersDrawer;
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::sleep::{end_of_chapter_seconds, sleep_toolbar_label, use_sleep_timer};
use super::sleep_panel::SleepPanel;
use super::speed_panel::SpeedPanel;
use super::stage::{
    PlaybackPosition, PlayerCallbacks, PlayerContent, PlayerStage, ToolbarState, TransportState,
};
use crate::Nav;

/// Derive the current chapter index from `elapsed` and a sorted chapter list.
/// Returns 0 when `chapters` is empty.
pub(super) fn chapter_index_for_elapsed(chapters: &[ChapterInfo], elapsed: f64) -> usize {
    if chapters.is_empty() {
        return 0;
    }
    chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1)
}

/// Resolve the seek target for "previous chapter":
/// - If we're more than 3 s into the current chapter, seek to its start.
/// - If within 3 s of the start and not the first chapter, go to the previous.
/// - Otherwise seek to 0.
///
/// Returns `None` when `chapters` is empty or `idx` is out of bounds
/// (the click handler computes `idx` from a separate signal, so a chapter
/// list refresh in between can leave it stale).
pub(super) fn chapter_prev_target(
    chapters: &[ChapterInfo],
    elapsed: f64,
    idx: usize,
) -> Option<f64> {
    let current = chapters.get(idx)?;
    let target = if elapsed - current.start_seconds > 3.0 {
        current.start_seconds
    } else if let Some(prev) = idx.checked_sub(1).and_then(|i| chapters.get(i)) {
        prev.start_seconds
    } else {
        0.0
    };
    Some(target)
}

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
    use omnibus_shared::ChapterInfo;

    use super::*;

    fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    #[test]
    fn chapter_index_for_elapsed_returns_zero_for_empty_list() {
        assert_eq!(chapter_index_for_elapsed(&[], 60.0), 0);
    }

    #[test]
    fn chapter_index_for_elapsed_returns_first_chapter_before_any_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_index_for_elapsed(&chs, 0.0), 0);
        assert_eq!(chapter_index_for_elapsed(&chs, 150.0), 0);
    }

    #[test]
    fn chapter_index_for_elapsed_advances_past_chapter_boundary() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_index_for_elapsed(&chs, 300.0), 1);
        assert_eq!(chapter_index_for_elapsed(&chs, 500.0), 1);
    }

    #[test]
    fn chapter_prev_target_returns_none_when_chapters_empty() {
        assert_eq!(chapter_prev_target(&[], 10.0, 0), None);
    }

    #[test]
    fn chapter_prev_target_returns_chapter_start_when_well_into_chapter() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // 50 s into chapter 1 (idx=1, start=300), elapsed=350 → 350-300=50 > 3 → back to 300
        assert_eq!(chapter_prev_target(&chs, 350.0, 1), Some(300.0));
    }

    #[test]
    fn chapter_prev_target_returns_previous_chapter_when_near_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // 1 s into chapter 1 (idx=1, start=300), elapsed=301 → 301-300=1 ≤ 3 → go to ch 0 start=0
        assert_eq!(chapter_prev_target(&chs, 301.0, 1), Some(0.0));
    }

    #[test]
    fn chapter_prev_target_returns_zero_when_at_first_chapter_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        assert_eq!(chapter_prev_target(&chs, 1.0, 0), Some(0.0));
    }

    #[test]
    fn chapter_prev_target_returns_none_when_idx_is_out_of_bounds() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        // idx came from a stale signal — chapters list shrunk under it.
        assert_eq!(chapter_prev_target(&chs, 1.0, 5), None);
    }

    #[test]
    fn scrub_max_returns_duration_when_positive() {
        assert!((scrub_max(120.5) - 120.5).abs() < f64::EPSILON);
    }

    #[test]
    fn scrub_max_returns_one_when_duration_is_zero() {
        assert!((scrub_max(0.0) - 1.0).abs() < f64::EPSILON);
    }
}

/// Grouped playback signals (duration, elapsed, playing, rate, hls_ready).
#[derive(Clone, Copy, PartialEq)]
pub(super) struct PlaybackSignals {
    pub duration: Signal<f64>,
    pub elapsed: Signal<f64>,
    pub playing: Signal<bool>,
    pub rate: Signal<f64>,
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
    let duration = signals.duration;
    let elapsed = signals.elapsed;
    let playing = signals.playing;
    let rate = signals.rate;
    let hls_ready = signals.hls_ready;
    let mut speed_panel_open = use_signal(|| false);
    let mut sleep_panel_open = use_signal(|| false);
    let mut bookmarks_open = use_signal(|| false);
    let mut chapters_open = use_signal(|| false);

    // Sleep timer + bookmark state. Both are custom hooks declared
    // unconditionally so SSR/WASM hook order matches (rule 07); their web
    // interop is gated internally.
    let sleep = use_sleep_timer();
    let bookmarks = use_bookmarks(uuid.clone());

    // Derive current chapter index from elapsed position.
    let current_chapter_index = use_memo(move || {
        let chs = chapters();
        let elapsed_now = elapsed();
        chapter_index_for_elapsed(&chs, elapsed_now)
    });

    let on_toggle = super::helpers::on_toggle_playback();
    let on_skip_back = super::helpers::on_skip_back_30();
    let on_skip_forward = super::helpers::on_skip_forward_30();
    let on_seek = move |evt: Event<FormData>| {
        if let Ok(_secs) = evt.value().parse::<f64>() {
            #[cfg(feature = "web")]
            super::helpers::audio_call("seek", &_secs.to_string());
        }
    };
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
        if let Some(_target) = chapter_prev_target(&chs, elapsed(), idx) {
            #[cfg(feature = "web")]
            super::helpers::audio_call("seek", &_target.to_string());
        }
    };

    let on_chapter_next = move |_: MouseEvent| {
        let chs = chapters();
        let idx = current_chapter_index();
        if idx + 1 < chs.len() {
            #[cfg(feature = "web")]
            super::helpers::audio_call("seek", &chs[idx + 1].start_seconds.to_string());
        }
    };

    let on_chapter_seek = move |_secs: f64| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("seek", &_secs.to_string());
    };

    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string());
    let dur = duration();
    let elapsed_now = elapsed();
    let remaining = (dur - elapsed_now).max(0.0);
    let scrub_max = scrub_max(dur);
    let rate_label = format!("{:.2}\u{00d7}", rate());
    let play_label = if playing() { "Pause" } else { "Play" }.to_string();
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
    let sleep_choice = (sleep.choice)();
    let sleep_fade = (sleep.fade)();
    let sleep_active = matches!(sleep_remaining, Some(s) if s > 0);
    let sleep_btn_label = sleep_toolbar_label(sleep_remaining);
    let has_chapters = !chs.is_empty();
    let bookmark_toast = (bookmarks.toast)();

    rsx! {
        div { class: "lp-root", style: "{accent_style}",
            div { class: "lp-backdrop" }

            Nav {}

            if failed {
                FailedOverlay {}
            } else if !ready {
                PreparingOverlay {}
            }

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
                    remaining,
                    scrub_max,
                    current_chapter_index: ch_idx,
                },
                transport: TransportState {
                    play_label,
                    playing: playing(),
                    rate_label,
                    rate_active: speed_panel_open(),
                    has_chapters: !chs.is_empty(),
                },
                callbacks: PlayerCallbacks {
                    on_seek: EventHandler::new(on_seek),
                    on_toggle: EventHandler::new(on_toggle),
                    on_skip_back: EventHandler::new(on_skip_back),
                    on_skip_forward: EventHandler::new(on_skip_forward),
                    on_rate: EventHandler::new(on_rate),
                    on_chapter_prev: EventHandler::new(on_chapter_prev),
                    on_chapter_next: EventHandler::new(on_chapter_next),
                    on_chapter_seek: EventHandler::new(on_chapter_seek),
                },
                toolbar: ToolbarState {
                    sleep_active: sleep_active || sleep_panel_open(),
                    sleep_label: sleep_btn_label,
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

            if speed_panel_open() {
                SpeedPanel {
                    rate,
                    uuid: uuid.clone(),
                    on_close: move |_| speed_panel_open.set(false),
                }
            }
            if sleep_panel_open() {
                SleepPanel {
                    remaining: sleep_remaining,
                    choice: sleep_choice,
                    fade: sleep_fade,
                    has_chapters,
                    on_select: move |secs: i32| sleep.select_seconds(secs),
                    on_end_of_chapter: move |_| {
                        let chs_now = chapters.peek().clone();
                        let idx = current_chapter_index();
                        if let Some(secs) = end_of_chapter_seconds(&chs_now, idx, *elapsed.peek()) {
                            sleep.select_end_of_chapter(secs);
                        }
                    },
                    on_toggle_fade: move |_| sleep.toggle_fade(),
                    on_close: move |_| sleep_panel_open.set(false),
                }
            }
            if bookmarks_open() {
                BookmarksDrawer {
                    controller: bookmarks,
                    chapters: chs.clone(),
                    on_seek: EventHandler::new(on_chapter_seek),
                    on_add: EventHandler::new(move |_| {
                        let chs_now = chapters.peek().clone();
                        bookmarks.create(*elapsed.peek(), &chs_now);
                    }),
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
                    chapters: chs.clone(),
                    current_chapter_index: ch_idx,
                    elapsed: elapsed_now,
                    on_seek: EventHandler::new(on_chapter_seek),
                    on_close: move |_| chapters_open.set(false),
                }
            }
        }
    }
}
