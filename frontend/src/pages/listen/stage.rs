//! Two-column player stage: cover on the left, title / author / scrubber /
//! transport on the right. Receives the resolved display data + per-action
//! [`EventHandler`]s so the parent orchestrator stays under the component
//! line-count cap.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::controls::{Scrubber, Toolbar, TransportButtons};
use crate::components::atrium::Cover;

/// Scrubber timing: current position plus the derived totals the bar
/// renders alongside it (duration, remaining, and the slider's max).
#[derive(Clone, PartialEq)]
pub(super) struct PlaybackPosition {
    pub elapsed: f64,
    pub duration: f64,
    pub remaining: f64,
    pub scrub_max: f64,
}

/// Play/skip/rate button labels and on/off state shared by the
/// transport row.
#[derive(Clone, PartialEq)]
pub(super) struct TransportState {
    pub play_label: String,
    pub playing: bool,
    pub rate_label: String,
    pub rate_active: bool,
}

/// Transport-row event handlers wired through to the scrubber + play /
/// skip / rate buttons.
#[derive(Clone, PartialEq)]
pub(super) struct PlayerCallbacks {
    pub on_seek: EventHandler<Event<FormData>>,
    pub on_toggle: EventHandler<MouseEvent>,
    pub on_skip_back: EventHandler<MouseEvent>,
    pub on_skip_forward: EventHandler<MouseEvent>,
    pub on_rate: EventHandler<MouseEvent>,
}

/// Toolbar row (sleep / bookmark / chapters): per-button highlight state
/// plus the toggle handlers their containing panels listen on.
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
) -> Element {
    let fill_pct = if position.scrub_max > 0.0 {
        (position.elapsed / position.scrub_max * 100.0).min(100.0)
    } else {
        0.0
    };

    rsx! {
        div { class: "lp-stage",
            div { class: "lp-cover-col",
                div { class: "lp-cover-ring",
                    Cover { book: book.clone() }
                }
            }
            div { class: "lp-info-col",
                div { class: "lp-kicker", "Now playing" }
                h1 { class: "lp-title", "{title}" }
                div { class: "lp-author", "by {author}" }
                div { class: "lp-chapter-sub" }

                Scrubber {
                    elapsed: position.elapsed,
                    duration: position.duration,
                    remaining: position.remaining,
                    scrub_max: position.scrub_max,
                    fill_pct,
                    on_seek: callbacks.on_seek,
                }

                TransportButtons {
                    play_label: transport.play_label,
                    playing: transport.playing,
                    rate_label: transport.rate_label,
                    rate_active: transport.rate_active,
                    on_toggle: callbacks.on_toggle,
                    on_skip_back: callbacks.on_skip_back,
                    on_skip_forward: callbacks.on_skip_forward,
                    on_rate: callbacks.on_rate,
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
