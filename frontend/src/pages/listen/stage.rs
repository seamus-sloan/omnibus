//! Two-column player stage: cover on the left, title / author / scrubber /
//! transport on the right. Receives the resolved display data + per-action
//! [`EventHandler`]s so the parent orchestrator stays under the component
//! line-count cap.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;

use super::controls::{Scrubber, TransportButtons};

/// Right-column "now playing" panel: title, author, scrubber, transport row.
///
/// `book` is cloned into the [`Cover`] so the panel can stand on its own;
/// the parent passes the same metadata into the cover column.
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
    rate_label: String,
    on_seek: EventHandler<Event<FormData>>,
    on_toggle: EventHandler<MouseEvent>,
    on_skip_back: EventHandler<MouseEvent>,
    on_skip_forward: EventHandler<MouseEvent>,
    on_rate: EventHandler<MouseEvent>,
) -> Element {
    rsx! {
        div {
            class: "player-stage",
            style: "flex:1; min-height:0; display:grid; grid-template-columns: 1fr 1fr; align-items:center; gap:48px; padding:0 48px;",
            div {
                style: "display:grid; place-items:center;",
                div { style: "width:min(380px, 80%);",
                    Cover { book: book.clone() }
                }
            }
            div {
                h1 {
                    class: "player-title",
                    style: "font-family:var(--serif); font-style:italic; font-size:clamp(36px, 6vw, 72px); line-height:0.95; margin:0;",
                    "{title}"
                }
                div { style: "margin-top:12px; color:var(--ink-1); font-size:16px;",
                    "by {author}"
                }

                Scrubber {
                    elapsed,
                    duration,
                    remaining,
                    scrub_max,
                    on_seek,
                }

                TransportButtons {
                    play_label,
                    rate_label,
                    on_toggle,
                    on_skip_back,
                    on_skip_forward,
                    on_rate,
                }
            }
        }
    }
}
