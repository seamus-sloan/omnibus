//! Post-load player chrome: top nav, hidden `<audio>`, status overlays,
//! and the two-column [`PlayerStage`]. Owns the per-action handlers
//! (back / toggle / skip / seek / rate) and overlay open/close state.
//! Rendered by the orchestrator after the `loading` / `error` /
//! `book.is_none()` gates.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::bookmarks_drawer::BookmarksDrawer;
use super::chapters_drawer::ChaptersDrawer;
use super::controls::AudioElement;
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::sleep_panel::SleepPanel;
use super::speed_panel::SpeedPanel;
use super::stage::{PlaybackPosition, PlayerCallbacks, PlayerStage, ToolbarState, TransportState};
use crate::Nav;

/// Render the ready-state player chrome and bind the transport handlers.
#[allow(clippy::too_many_lines)]
#[component]
pub(super) fn ReadyPlayer(
    book: EbookMetadata,
    uuid: String,
    duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    rate: Signal<f64>,
    hls_ready: Signal<bool>,
    playback_failed: Signal<bool>,
    chapters: Signal<Vec<ChapterInfo>>,
) -> Element {
    let mut speed_panel_open = use_signal(|| false);
    let mut sleep_panel_open = use_signal(|| false);
    let mut bookmarks_open = use_signal(|| false);
    let mut chapters_open = use_signal(|| false);

    // Derive current chapter index from elapsed position.
    let current_chapter_index = use_memo(move || {
        let chs = chapters();
        let elapsed_now = elapsed();
        if chs.is_empty() {
            return 0usize;
        }
        chs.partition_point(|c| c.start_seconds <= elapsed_now)
            .saturating_sub(1)
    });

    let on_toggle = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("toggle", "");
    };
    let on_skip_back = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "-30");
    };
    let on_skip_forward = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "30");
    };
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
        if chs.is_empty() {
            return;
        }
        let _target = if elapsed() - chs[idx].start_seconds > 3.0 {
            chs[idx].start_seconds
        } else if idx > 0 {
            chs[idx - 1].start_seconds
        } else {
            0.0
        };
        #[cfg(feature = "web")]
        super::helpers::audio_call("seek", &_target.to_string());
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
    let scrub_max = if dur > 0.0 { dur } else { 1.0 };
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

    rsx! {
        div { class: "lp-root", style: "{accent_style}",
            div { class: "lp-backdrop" }

            Nav {}
            AudioElement {}

            if failed {
                FailedOverlay {}
            } else if !ready {
                PreparingOverlay {}
            }

            PlayerStage {
                book: book.clone(),
                title,
                author,
                position: PlaybackPosition {
                    elapsed: elapsed_now,
                    duration: dur,
                    remaining,
                    scrub_max,
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
                    sleep_active: sleep_panel_open(),
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
                chapters: chs.clone(),
                current_chapter_index: ch_idx,
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
                    on_close: move |_| sleep_panel_open.set(false),
                }
            }
            if bookmarks_open() {
                BookmarksDrawer {
                    on_close: move |_| bookmarks_open.set(false),
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
