//! The scanner screen — the check-in flow's front door on the mobile shell,
//! and one explicit button away everywhere else.
//!
//! Thin chrome around the reusable [`BarcodeScanner`]: the flow owns what a
//! decode means, the component owns the camera. Visible text here is the
//! E2E selector contract — keep it stable (rule 04).

use dioxus::prelude::*;

use super::lookup::ProviderNote;
use crate::components::BarcodeScanner;

/// Live camera scan. `on_detect` carries a confirmed EAN-13; `on_manual` is
/// the always-available keypad escape hatch, which the scanner also promotes
/// to the primary action when the camera is unavailable.
#[component]
pub(super) fn ScanScreen(on_detect: EventHandler<String>, on_manual: EventHandler<()>) -> Element {
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-scan",
            h1 { "Scan a barcode" }
            p { class: "subtitle",
                "Hold the back cover in the frame \u{2014} we'll read the ISBN automatically."
            }
            BarcodeScanner { on_detect, on_manual }
            ProviderNote {}
        }
    }
}
