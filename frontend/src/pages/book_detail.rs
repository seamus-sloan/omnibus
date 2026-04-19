use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Stub detail page for a single book. `id` is the stable backend id from
/// the `books` table (`EbookMetadata.id`), so deep links survive re-indexes
/// and the detail page can later fetch by id without depending on
/// landing-page row ordering.
#[component]
pub fn BookDetailPage(id: i64) -> Element {
    rsx! {
        section { class: "card",
            h1 { "Book #{id}" }
            p { class: "subtitle", "Book detail page — TODO." }
            Link { to: Route::Landing {}, class: "btn", "Back to library" }
        }
    }
}
