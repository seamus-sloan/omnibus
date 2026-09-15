//! Shelves index (`/shelves`). On web: every visible shelf as an owner-grouped
//! card grid with a text filter, owner and kind toggles, and a New shelf
//! action ([`web`]). In the native shell: the full-screen list reached from
//! the library header ([`mobile`]).

use dioxus::prelude::*;

#[cfg(not(feature = "mobile"))]
mod card;
#[cfg(not(feature = "mobile"))]
mod filter;
#[cfg(feature = "mobile")]
mod mobile;
#[cfg(not(feature = "mobile"))]
mod web;

#[cfg(all(test, not(feature = "mobile")))]
mod tests;

/// Shelves index — see the module doc for the per-target split. (Mobile is a
/// separate build, so rule 07's SSR/WASM parity is unaffected by the split.)
#[component]
pub fn ShelvesIndexPage() -> Element {
    #[cfg(feature = "mobile")]
    {
        rsx! { mobile::MobileShelvesIndex {} }
    }
    #[cfg(not(feature = "mobile"))]
    {
        rsx! { web::WebShelvesIndex {} }
    }
}
