//! Mobile-only (CSS-gated via `.screen`) discovery "back" affordance — a
//! single `←` icon button in the top-left of a detail page's header, linking
//! to its parent index. Rendered identically on every target so the web
//! SSR/WASM trees stay in parity (rule 07); shared by the series, author,
//! and tag-cloud detail headers.

use dioxus::prelude::*;
use dioxus_router::Link;

use crate::Route;

/// The `←` back-to-index link. `aria_label` and `testid` are per-page (e.g.
/// "Back to series" / `series-back`).
pub fn disc_back_link(to: Route, aria_label: &str, testid: &str) -> Element {
    rsx! {
        Link {
            to,
            class: "m-icon-btn disc-back",
            "aria-label": "{aria_label}",
            "data-testid": "{testid}",
            "\u{2190}"
        }
    }
}
