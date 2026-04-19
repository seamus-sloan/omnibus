use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Stub detail page for a single book. The landing page table links each
/// row here so clicks and keyboard activation work end-to-end. The id is
/// the book's index in the current landing-page listing — this will be
/// replaced once the backend exposes stable book ids.
#[component]
pub fn BookDetailPage(id: usize) -> Element {
    rsx! {
        section { class: "card",
            h1 { "Book #{id}" }
            p { class: "subtitle", "Book detail page — TODO." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        }
    }
}
