//! Listen-page overlays for terminal failure and HLS-transcode preparation.
//!
//! Pure presentation — the parent passes booleans for the active state and
//! the overlays render above the player stage.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

// Pure presentational — no branching logic to unit-test here.
// Overlay visibility is controlled by the booleans in `ready_player` (already
// tested) and rendered output is covered by Playwright at
// ui_tests/playwright/tests/flows/.

/// Terminal failure overlay shown when the HLS `.failed` marker is present
/// or the manifest fetch failed outright.
#[component]
pub(super) fn FailedOverlay() -> Element {
    rsx! {
        div {
            class: "lp-overlay",
            "data-testid": "listen-failed",
            role: "alert",
            p { class: "lp-overlay-title", "Playback failed." }
            p { class: "lp-overlay-detail",
                "The audiobook could not be prepared. Reload the page to try again, or check the server logs for a transcode failure."
            }
        }
    }
}

/// HLS-transcode preparing overlay. Direct-play books flip `ready` true as
/// soon as the manifest fetch returns, so this only ever renders for the
/// HLS fallback path.
#[component]
pub(super) fn PreparingOverlay() -> Element {
    rsx! {
        div {
            class: "lp-overlay",
            "data-testid": "listen-preparing",
            p { class: "lp-overlay-title", "Preparing your audiobook\u{2026}" }
            p { class: "lp-overlay-detail",
                "This may take a moment on first listen."
            }
        }
    }
}
