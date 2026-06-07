//! Two-column player stage: cover on the left, title / author / chapter map /
//! transport on the right. Receives the resolved display data + per-action
//! [`EventHandler`]s so the parent orchestrator stays under the component
//! line-count cap.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::chapter_map::ChapterMap;
use super::controls::{Toolbar, TransportButtons};
use crate::components::atrium::Cover;

/// Scrubber timing — elapsed plus the derived totals (duration, remaining, slider max).
#[derive(Clone, PartialEq)]
pub(super) struct PlaybackPosition {
    pub elapsed: f64,
    pub duration: f64,
    pub remaining: f64,
    pub scrub_max: f64,
}

/// Play/skip/rate button labels and on/off state shared by the transport row.
#[derive(Clone, PartialEq)]
pub(super) struct TransportState {
    pub play_label: String,
    pub playing: bool,
    pub rate_label: String,
    pub rate_active: bool,
    pub has_chapters: bool,
}

/// Transport-row event handlers wired through to the scrubber + play/skip/rate buttons.
#[derive(Clone, PartialEq)]
pub(super) struct PlayerCallbacks {
    pub on_seek: EventHandler<Event<FormData>>,
    pub on_toggle: EventHandler<MouseEvent>,
    pub on_skip_back: EventHandler<MouseEvent>,
    pub on_skip_forward: EventHandler<MouseEvent>,
    pub on_rate: EventHandler<MouseEvent>,
    pub on_chapter_prev: EventHandler<MouseEvent>,
    pub on_chapter_next: EventHandler<MouseEvent>,
    pub on_chapter_seek: EventHandler<f64>,
}

/// Toolbar row (sleep/bookmark/chapters) — per-button highlight state plus toggle handlers.
#[derive(Clone, PartialEq)]
pub(super) struct ToolbarState {
    pub sleep_active: bool,
    pub bookmarks_active: bool,
    pub chapters_active: bool,
    pub on_sleep: EventHandler<MouseEvent>,
    pub on_bookmark: EventHandler<MouseEvent>,
    pub on_chapters: EventHandler<MouseEvent>,
}

#[component]
pub(super) fn PlayerStage(
    book: EbookMetadata,
    title: String,
    author: String,
    position: PlaybackPosition,
    transport: TransportState,
    callbacks: PlayerCallbacks,
    toolbar: ToolbarState,
    chapters: Vec<ChapterInfo>,
    current_chapter_index: usize,
) -> Element {
    let ch_count = chapters.len();
    let kicker = if ch_count > 0 {
        format!(
            "Now playing \u{00b7} Chapter {} of {}",
            current_chapter_index + 1,
            ch_count
        )
    } else {
        "Now playing".to_string()
    };

    let chapter_sub = chapters
        .get(current_chapter_index)
        .filter(|c| !c.title.is_empty())
        .map(|c| format!("Ch. {} \u{00b7} {}", current_chapter_index + 1, c.title));

    rsx! {
        div { class: "lp-stage",
            div { class: "lp-cover-col",
                div { class: "lp-cover-ring",
                    Cover { book: book.clone() }
                }
            }
            div { class: "lp-info-col",
                div { class: "lp-kicker", "{kicker}" }
                h1 { class: "lp-title", "{title}" }
                div { class: "lp-author", "by {author}" }
                if let Some(sub) = chapter_sub {
                    div { class: "lp-chapter-sub", "{sub}" }
                }

                ChapterMap {
                    chapters: chapters.clone(),
                    elapsed: position.elapsed,
                    duration: position.duration,
                    remaining: position.remaining,
                    current_chapter_index,
                    on_seek: callbacks.on_chapter_seek,
                }

                TransportButtons {
                    play_label: transport.play_label,
                    playing: transport.playing,
                    rate_label: transport.rate_label,
                    rate_active: transport.rate_active,
                    has_chapters: transport.has_chapters,
                    on_toggle: callbacks.on_toggle,
                    on_skip_back: callbacks.on_skip_back,
                    on_skip_forward: callbacks.on_skip_forward,
                    on_rate: callbacks.on_rate,
                    on_chapter_prev: callbacks.on_chapter_prev,
                    on_chapter_next: callbacks.on_chapter_next,
                }

                Toolbar {
                    sleep_active: toolbar.sleep_active,
                    bookmarks_active: toolbar.bookmarks_active,
                    chapters_active: toolbar.chapters_active,
                    on_sleep: toolbar.on_sleep,
                    on_bookmark: toolbar.on_bookmark,
                    on_chapters: toolbar.on_chapters,
                }
            }
        }
    }
}
