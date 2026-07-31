//! Shared page-level loading / error / not-found states. Nearly every
//! top-level page in `pages/` hand-duplicated the same `<p class="subtitle">`
//! / `role="alert"` markup around its own data-fetch effect; these three
//! components are drop-in replacements that preserve the exact role/text
//! contract existing Playwright specs assert on.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Loading placeholder shown while a page's data fetch is in flight.
#[component]
pub fn PageLoading() -> Element {
    rsx! {
        p { class: "subtitle", "Loading\u{2026}" }
    }
}

/// Error state: the fetch failure `message` plus a link back to `back_to`.
#[component]
pub fn PageError(
    message: String,
    back_to: Route,
    #[props(default = "Back to library".to_string())] back_label: String,
) -> Element {
    rsx! {
        p { role: "alert", class: "subtitle", "{message}" }
        Link { to: back_to, class: "btn", "{back_label}" }
    }
}

/// Not-found state: "`{subject}` not found." plus a link back to `back_to`.
#[component]
pub fn PageNotFound(
    subject: String,
    back_to: Route,
    #[props(default = "Back to library".to_string())] back_label: String,
) -> Element {
    rsx! {
        p { class: "subtitle", "{subject} not found." }
        Link { to: back_to, class: "btn", "{back_label}" }
    }
}

// `PageError`/`PageNotFound` both render a `dioxus_router::Link`, which
// panics without a live `RouterContext` — only obtainable by mounting
// `dioxus_router::Router`, and there's no route in this crate's `Route` enum
// that resolves to a bare `PageError`/`PageNotFound` to mount it through.
// This is the same constraint that keeps `TopNav`'s full render untested
// (`components::top_nav`): Dioxus catches the panic per-component (it
// doesn't abort the whole render), so the assertions below on the
// surrounding, Link-free markup still hold — only the link's own text is
// unverifiable here.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render;

    #[test]
    fn page_loading_renders_the_shared_subtitle_placeholder() {
        let html = render(rsx! { PageLoading {} });
        assert!(html.contains("class=\"subtitle\""));
        assert!(html.contains("Loading"));
    }

    #[test]
    fn page_error_renders_the_message_as_an_alert() {
        let html = render(rsx! {
            PageError {
                message: "Could not load this book.".to_string(),
                back_to: Route::Landing {},
            }
        });
        assert!(html.contains("role=\"alert\""));
        assert!(html.contains("Could not load this book."));
    }

    #[test]
    fn page_not_found_renders_the_subject() {
        let html = render(rsx! {
            PageNotFound {
                subject: "This book".to_string(),
                back_to: Route::Landing {},
            }
        });
        assert!(html.contains("This book not found."));
    }
}
