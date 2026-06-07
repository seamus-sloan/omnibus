//! Two-column player stage: cover on the left, title / author / scrubber /
//! transport on the right. Receives the resolved display data + per-action
//! [`EventHandler`]s so the parent orchestrator stays under the component
//! line-count cap.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::controls::{Scrubber, Toolbar, TransportButtons};
use crate::components::atrium::Cover;

#[allow(clippy::too_many_arguments)]
#[component]
pub(super) fn PlayerStage(
    book: EbookMetadata,
    title: String,
    author: String,
    elapsed: f64,
    duration: f64,
    remaining: f64,
    scrub_max: f64,
    play_label: String,
    playing: bool,
    rate_label: String,
    rate_active: bool,
    on_seek: EventHandler<Event<FormData>>,
    on_toggle: EventHandler<MouseEvent>,
    on_skip_back: EventHandler<MouseEvent>,
    on_skip_forward: EventHandler<MouseEvent>,
    on_rate: EventHandler<MouseEvent>,
    sleep_active: bool,
    bookmarks_active: bool,
    chapters_active: bool,
    on_sleep: EventHandler<MouseEvent>,
    on_bookmark: EventHandler<MouseEvent>,
    on_chapters: EventHandler<MouseEvent>,
) -> Element {
    let fill_pct = if scrub_max > 0.0 {
        (elapsed / scrub_max * 100.0).min(100.0)
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
                    elapsed,
                    duration,
                    remaining,
                    scrub_max,
                    fill_pct,
                    on_seek,
                }

                TransportButtons {
                    play_label,
                    playing,
                    rate_label,
                    rate_active,
                    on_toggle,
                    on_skip_back,
                    on_skip_forward,
                    on_rate,
                }

                Toolbar {
                    sleep_active,
                    bookmarks_active,
                    chapters_active,
                    on_sleep,
                    on_bookmark,
                    on_chapters,
                }
            }
        }
    }
}
