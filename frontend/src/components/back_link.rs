//! Mobile-only discovery "back" affordance — a single `←` icon button in the
//! top-left of a detail page's header, linking to its parent index. Shared by
//! the series, author, and tag-cloud detail headers.
//!
//! Emitted on the mobile shell alone. The web shell reaches its indexes
//! through the top nav, and a link the CSS hides at every viewport is an
//! affordance the markup advertises but the page does not have (#2291). The
//! gate is `mobile` rather than `web`, so both halves of the web build — SSR
//! and the WASM client — render the same nothing (rule 07).

use dioxus::prelude::*;
#[cfg(feature = "mobile")]
use dioxus_router::Link;

use crate::Route;

/// The `←` back-to-index link. `aria_label` and `testid` are per-page (e.g.
/// "Back to series" / `series-back`).
#[cfg(feature = "mobile")]
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

/// Web/server stub: the top nav is the way back, so nothing is emitted.
#[cfg(not(feature = "mobile"))]
pub fn disc_back_link(_to: Route, _aria_label: &str, _testid: &str) -> Element {
    rsx! {}
}
