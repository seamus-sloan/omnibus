//! Listen-page overlays for terminal failure and HLS-transcode preparation.
//!
//! Pure presentation — the parent passes booleans for the active state and
//! the overlays render above the player stage.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

// Pure presentational — no branching logic to unit-test here.
// Overlay visibility is gated by booleans owned in `ready_player`; rendered
// output is exercised by ui_tests/playwright/tests/flows/listen.spec.ts
// (preparing + failed states). Unit coverage of the boolean wiring itself
// would need component-render infra and is intentionally not in scope.

/// Terminal failure overlay shown when the HLS `.failed` marker is present,
/// the manifest fetch failed outright, the JS bootstrap never installed
/// (`__omnibusOnInitTimeout`), or a load stalled with no forward progress
/// for too long (`__omnibusOnAudioStalled`) — all four reuse
/// `playback_failed` since none of them recover without a reload.
#[component]
pub(super) fn FailedOverlay() -> Element {
    rsx! {
        div {
            class: "lp-overlay",
            "data-testid": "listen-failed",
            role: "alert",
            p { class: "lp-overlay-title", "Playback failed." }
            p { class: "lp-overlay-detail",
                "The audiobook could not be prepared or kept stalling. Reload the page to try again, or check the server logs for a transcode failure."
            }
        }
    }
}

/// HLS-transcode preparing overlay. Direct-play books flip `ready` true as
/// soon as the JS bootstrap confirms (`__omnibusOnAudioBooted`) that
/// `initDirect` committed a source, so this only ever renders for the HLS
/// fallback path.
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
