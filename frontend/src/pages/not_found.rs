//! Catch-all page for a URL no route matches.
//!
//! Without it dioxus-router renders its own parse diagnostic — the whole
//! internal route table, admin paths included — as the page body, with no
//! nav and no way back. Mounted through [`crate::ScreenLayout`] so the
//! shared chrome is there to escape with.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Renders the not-found page for `segments`, the unmatched path.
#[component]
pub fn NotFoundPage(segments: Vec<String>) -> Element {
    // The path is echoed so a mistyped URL is self-explanatory. It is bound
    // as text (never `dangerous_inner_html`), so a crafted path renders as
    // the literal characters a reader typed.
    let path = format!("/{}", segments.join("/"));
    rsx! {
        div { class: "disc-page", "data-testid": "not-found-page",
            div { class: "disc-body",
                h1 { class: "disc-hero-title", "Page not found" }
                p { class: "subtitle", "Nothing lives at {path}." }
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}

// `NotFoundPage` renders a `dioxus_router::Link`, which panics without a live
// `RouterContext`. Dioxus catches that per-component rather than aborting the
// render, so the assertions below on the surrounding markup still hold — only
// the link's own text is unverifiable here (same constraint as
// `components::page_state`).
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render;

    #[test]
    fn not_found_page_names_the_unmatched_path() {
        let html = render(rsx! {
            NotFoundPage { segments: vec!["add".to_string()] }
        });
        assert!(html.contains("Page not found"), "{html}");
        assert!(html.contains("/add"), "{html}");
    }

    #[test]
    fn not_found_page_never_lists_the_internal_routes() {
        // The router's own diagnostic enumerated every route, admin paths
        // included, to anyone who mistyped a URL (#2214).
        let html = render(rsx! {
            NotFoundPage { segments: vec!["nope".to_string()] }
        });
        assert!(!html.contains("Attempted Matches"), "{html}");
        assert!(!html.contains("admin/health"), "{html}");
    }

    #[test]
    fn not_found_page_escapes_the_path_it_echoes() {
        let html = render(rsx! {
            NotFoundPage { segments: vec!["<script>".to_string()] }
        });
        assert!(!html.contains("<script>"), "{html}");
    }
}
