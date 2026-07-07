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
