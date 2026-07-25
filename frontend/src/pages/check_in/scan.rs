//! The scanner screen — the check-in flow's front door.
//!
//! Thin chrome around the reusable [`BarcodeScanner`]: the flow owns what a
//! decode means, the component owns the camera. Visible text here is the
//! Maestro selector contract — keep it stable (rule 04, Mobile E2E).

use dioxus::prelude::*;

use crate::components::BarcodeScanner;
use crate::{data, use_server_url};

/// Live camera scan. `on_detect` carries a confirmed EAN-13; `on_manual` is
/// the always-available keypad escape hatch, which the scanner also promotes
/// to the primary action when the camera is unavailable.
#[component]
pub(super) fn ScanScreen(on_detect: EventHandler<String>, on_manual: EventHandler<()>) -> Element {
    let server_url = use_server_url();
    // The masked key status is admin-only. On web we know the caller's admin
    // flag client-side, so we skip the fetch — and its 403 — for everyone else.
    // Mobile has no client-side admin signal (`use_is_admin` is always false
    // there), so it always attempts the fetch; a non-admin's 403 simply yields
    // no note. Either way the note shows only when an admin's server has no key.
    #[cfg(feature = "web")]
    let is_admin = crate::use_is_admin();
    // Starts `None` on both SSR and the first WASM paint, so the note only
    // appears after the post-mount fetch resolves (hydration parity, rule 07).
    let mut key_configured = use_signal(|| None::<bool>);
    use_effect(move || {
        #[cfg(feature = "web")]
        if !is_admin() {
            return;
        }
        let url = server_url.clone();
        spawn(async move {
            if let Ok(status) = data::get_google_books_key(&url).await {
                key_configured.set(Some(status.configured));
            }
        });
    });

    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-scan",
            h1 { "Scan a barcode" }
            p { class: "subtitle",
                "Hold the back cover in the frame \u{2014} we'll read the ISBN automatically."
            }
            BarcodeScanner { on_detect, on_manual }
            if key_configured() == Some(false) {
                p { class: "check-in-provider-note", "data-testid": "check-in-google-books-note",
                    "Without a Google Books API key, this search will use Open Library, which may not have the latest books or all ISBNs. Set up your Google Books API key in Settings."
                }
            }
        }
    }
}
