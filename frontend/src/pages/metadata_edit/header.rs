//! Breadcrumb + page-header subcomponents for the metadata edit form.
//!
//! Pure presentation: receives the resolved display title and primary
//! author and renders the bd-crumb nav + me-page-header block.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// Breadcrumb nav: Home › author › book › "Edit metadata".
#[component]
pub(super) fn Breadcrumb(
    uuid: String,
    display_title: String,
    primary_author: String,
    primary_author_id: Option<i64>,
) -> Element {
    rsx! {
        nav { class: "bd-crumb me-crumb", "aria-label": "breadcrumb",
            Link { to: Route::Landing {}, class: "bd-crumb-home", "Home" }
            span { class: "bd-crumb-sep", "\u{203a}" }
            if !primary_author.is_empty() {
                if let Some(author_id) = primary_author_id {
                    Link {
                        to: Route::AuthorDetail { id: author_id },
                        class: "bd-crumb-step",
                        "data-testid": "me-crumb-author",
                        "{primary_author}"
                    }
                } else {
                    span {
                        class: "bd-crumb-step",
                        "data-testid": "me-crumb-author",
                        "{primary_author}"
                    }
                }
                span { class: "bd-crumb-sep", "\u{203a}" }
            }
            Link { to: Route::BookDetail { uuid: uuid.clone() }, class: "bd-crumb-step", "{display_title}" }
            span { class: "bd-crumb-sep", "\u{203a}" }
            span { class: "bd-crumb-curr", "Edit metadata" }
        }
    }
}

/// Page header block with "Edit metadata" label, h2 title, and hint.
#[component]
pub(super) fn PageHeader(display_title: String, primary_author: String) -> Element {
    rsx! {
        div { class: "me-page-header",
            div {
                div {
                    class: "label",
                    "data-testid": "me-page-title-label",
                    "Edit metadata"
                }
                h2 { class: "me-page-title",
                    span { class: "me-page-title-book", "{display_title}" }
                    if !primary_author.is_empty() {
                        span { class: "me-page-title-author", "{primary_author}" }
                    }
                }
                div { class: "mono me-page-hint",
                    "changes apply on save"
                }
            }
        }
    }
}
