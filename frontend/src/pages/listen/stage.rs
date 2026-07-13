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

/// Build the "Now playing" kicker text, including chapter counter when available.
pub(super) fn kicker_text(ch_count: usize, current_chapter_index: usize) -> String {
    if ch_count > 0 {
        format!(
            "Now playing \u{00b7} Chapter {} of {}",
            current_chapter_index + 1,
            ch_count
        )
    } else {
        "Now playing".to_string()
    }
}

/// Build the chapter subtitle (e.g. "Ch. 2 · The Journey Begins") when the
/// current chapter has a non-empty title. Returns `None` when there are no
/// chapters or the chapter title is empty.
pub(super) fn chapter_sub_text(
    chapters: &[ChapterInfo],
    current_chapter_index: usize,
) -> Option<String> {
    chapters
        .get(current_chapter_index)
        .filter(|c| !c.title.is_empty())
        .map(|c| format!("Ch. {} \u{00b7} {}", current_chapter_index + 1, c.title))
}

#[cfg(test)]
mod tests {
    use omnibus_shared::ChapterInfo;

    use super::*;

    fn ch(ordinal: i64, title: &str, start: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: 300.0,
        }
    }

    #[test]
    fn kicker_text_without_chapters_returns_simple_label() {
        assert_eq!(kicker_text(0, 0), "Now playing");
    }

    #[test]
    fn kicker_text_with_chapters_includes_counter() {
        assert_eq!(kicker_text(5, 2), "Now playing \u{00b7} Chapter 3 of 5");
    }

    #[test]
    fn chapter_sub_text_returns_none_when_chapter_list_empty() {
        assert_eq!(chapter_sub_text(&[], 0), None);
    }

    #[test]
    fn chapter_sub_text_returns_none_when_chapter_title_empty() {
        let chs = vec![ch(1, "", 0.0)];
        assert_eq!(chapter_sub_text(&chs, 0), None);
    }

    #[test]
    fn chapter_sub_text_returns_formatted_title_when_present() {
        let chs = vec![ch(1, "Intro", 0.0), ch(2, "Part One", 300.0)];
        assert_eq!(
            chapter_sub_text(&chs, 1),
            Some("Ch. 2 \u{00b7} Part One".to_string())
        );
    }
}

/// Scrubber timing — elapsed plus the derived totals (duration, remaining,
/// slider max) — and the current chapter index.
#[derive(Clone, PartialEq)]
pub(super) struct PlaybackPosition {
    pub elapsed: f64,
    pub duration: f64,
    pub remaining: f64,
    pub scrub_max: f64,
    pub current_chapter_index: usize,
}

/// Static per-book display content: the book itself, title, author, and the full chapter list.
#[derive(Clone, PartialEq)]
pub(super) struct PlayerContent {
    pub book: EbookMetadata,
    pub title: String,
    pub author: String,
    pub chapters: Vec<ChapterInfo>,
}

/// Play/skip/rate button labels and on/off state shared by the transport row.
#[derive(Clone, PartialEq)]
pub(super) struct TransportState {
    pub play_label: String,
    pub playing: bool,
    pub rate_label: String,
    pub rate_active: bool,
    pub has_chapters: bool,
    /// Current volume (0.0–1.0) — drives the volume slider's thumb position.
    pub volume: f64,
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
    pub on_volume: EventHandler<f64>,
}

/// The subset of [`PlayerCallbacks`] the transport row itself dispatches.
#[derive(Clone, PartialEq)]
pub(super) struct TransportCallbacks {
    pub on_toggle: EventHandler<MouseEvent>,
    pub on_skip_back: EventHandler<MouseEvent>,
    pub on_skip_forward: EventHandler<MouseEvent>,
    pub on_rate: EventHandler<MouseEvent>,
    pub on_chapter_prev: EventHandler<MouseEvent>,
    pub on_chapter_next: EventHandler<MouseEvent>,
    pub on_volume: EventHandler<f64>,
}

/// Toolbar row (sleep/bookmark/chapters) — per-button highlight state plus toggle handlers.
#[derive(Clone, PartialEq)]
pub(super) struct ToolbarState {
    pub sleep_active: bool,
    pub sleep_label: String,
    pub bookmarks_active: bool,
    pub chapters_active: bool,
    pub on_sleep: EventHandler<MouseEvent>,
    pub on_bookmark: EventHandler<MouseEvent>,
    pub on_chapters: EventHandler<MouseEvent>,
}

#[component]
pub(super) fn PlayerStage(
    content: PlayerContent,
    position: PlaybackPosition,
    transport: TransportState,
    callbacks: PlayerCallbacks,
    toolbar: ToolbarState,
) -> Element {
    let PlayerContent {
        book,
        title,
        author,
        chapters,
    } = content;
    let ch_count = chapters.len();
    let kicker = kicker_text(ch_count, position.current_chapter_index);
    let chapter_sub = chapter_sub_text(&chapters, position.current_chapter_index);

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
                    current_chapter_index: position.current_chapter_index,
                    on_seek: callbacks.on_chapter_seek,
                }

                TransportButtons {
                    state: transport,
                    callbacks: TransportCallbacks {
                        on_toggle: callbacks.on_toggle,
                        on_skip_back: callbacks.on_skip_back,
                        on_skip_forward: callbacks.on_skip_forward,
                        on_rate: callbacks.on_rate,
                        on_chapter_prev: callbacks.on_chapter_prev,
                        on_chapter_next: callbacks.on_chapter_next,
                        on_volume: callbacks.on_volume,
                    },
                }

                Toolbar { state: toolbar }
            }
        }
    }
}
