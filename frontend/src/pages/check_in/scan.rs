//! The scanner screen — the check-in flow's front door.
//!
//! Thin chrome around the reusable [`BarcodeScanner`]: the flow owns what a
//! decode means, the component owns the camera. Visible text here is the
//! E2E selector contract — keep it stable (rule 04).

use dioxus::prelude::*;

use crate::components::BarcodeScanner;
use crate::{data, use_server_url};

/// Live camera scan. `on_detect` carries a confirmed EAN-13, `on_manual` opens
/// the keypad (promoted to the primary action when the camera is unavailable),
/// and `on_search` the title search.
#[component]
pub(super) fn ScanScreen(
    on_detect: EventHandler<String>,
    on_manual: EventHandler<()>,
    on_search: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    // `data::google_books_configured` is a non-admin-gated boolean check (any
    // authenticated user may read it), so every caller — web and mobile alike
    // — can fetch it directly with no role check and no guaranteed-403 round
    // trip for non-admins.
    // Starts `None` on both SSR and the first WASM paint, so the note only
    // appears after the post-mount fetch resolves (hydration parity, rule 07).
    let mut key_configured = use_signal(|| None::<bool>);
    use_effect(move || {
        let url = server_url.clone();
        spawn(async move {
            if let Ok(configured) = data::google_books_configured(&url).await {
                key_configured.set(Some(configured));
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
            div { class: "check-in-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "check-in-search-by-title",
                    onclick: move |_| on_search.call(()),
                    "Search by title"
                }
            }
            if key_configured() == Some(false) {
                p { class: "check-in-provider-note", "data-testid": "check-in-google-books-note",
                    "Without a Google Books API key, this search will use Open Library, which may not have the latest books or all ISBNs. Set up your Google Books API key in Settings."
                }
            }
        }
    }
}
