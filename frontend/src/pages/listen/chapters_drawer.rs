//! Chapters drawer for the listen page.
//!
//! Right-side full-height panel showing the table of contents. Currently
//! renders an empty state because chapter data requires a migration +
//! extraction pipeline (PR 2/3). The visual shell matches the design so
//! users can preview the layout.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Chapters drawer — empty state until chapter infrastructure ships.
#[component]
pub(super) fn ChaptersDrawer(on_close: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "lp-scrim",
            onclick: move |_| on_close.call(()),
        }

        div { class: "lp-drawer",
            div { class: "lp-drawer-head",
                div {
                    div { class: "lp-panel-kicker", "Chapters" }
                    div { class: "lp-panel-title", "Contents" }
                }
                button {
                    class: "btn ghost sm",
                    r#type: "button",
                    onclick: move |_| on_close.call(()),
                    "Close \u{2193}"
                }
            }
            div { class: "lp-drawer-body",
                div { class: "lp-drawer-empty",
                    p { class: "lp-drawer-empty-title", "No chapters yet" }
                    p { class: "lp-drawer-empty-detail",
                        "Chapter extraction is coming in a future update. "
                        "Books with embedded chapter markers will show their "
                        "table of contents here."
                    }
                }
            }
        }
    }
}
