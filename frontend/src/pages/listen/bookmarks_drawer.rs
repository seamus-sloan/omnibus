//! Bookmarks drawer for the listen page.
//!
//! Right-side full-height panel showing saved bookmarks. Currently
//! renders an empty state because the bookmarks CRUD endpoint ships
//! in PR 4. The visual shell matches the design.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Bookmarks drawer — empty state until bookmark persistence ships.
#[component]
pub(super) fn BookmarksDrawer(on_close: EventHandler<()>) -> Element {
    rsx! {
        div {
            class: "lp-scrim",
            onclick: move |_| on_close.call(()),
        }

        div { class: "lp-drawer", "data-testid": "bookmarks-drawer",
            div { class: "lp-drawer-head",
                div {
                    div { class: "lp-panel-kicker", "Bookmarks" }
                    div { class: "lp-panel-title", "Your marks" }
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
                    p { class: "lp-drawer-empty-title", "No bookmarks yet" }
                    p { class: "lp-drawer-empty-detail",
                        "Tap the Bookmark button while listening to save "
                        "your place. Bookmarks with notes will appear here."
                    }
                }
            }
        }
    }
}
