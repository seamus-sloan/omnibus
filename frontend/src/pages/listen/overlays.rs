//! Listen-page overlays for terminal failure and HLS-transcode preparation.
//!
//! Pure presentation — the parent passes booleans for the active state and
//! the overlays render at z-index 10 above the player stage.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Terminal failure overlay shown when the HLS `.failed` marker is present
/// or the manifest fetch failed outright. Distinct from "preparing" so a
/// manual reload (not just waiting) is the recovery path. Bug 4 from #338.
#[component]
pub(super) fn FailedOverlay() -> Element {
    rsx! {
        div {
            "data-testid": "listen-failed",
            role: "alert",
            style: "position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; background:var(--bg-0); z-index:10; gap:0.5rem;",
            p {
                style: "font-family:var(--serif); font-size:1.2rem; color:var(--ink-1);",
                "Playback failed."
            }
            p {
                style: "font-family:var(--mono); font-size:0.85rem; color:var(--ink-3); max-width:32rem; text-align:center;",
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
            "data-testid": "listen-preparing",
            style: "position:absolute; inset:0; display:flex; flex-direction:column; align-items:center; justify-content:center; background:var(--bg-0); z-index:10;",
            p {
                style: "font-family:var(--serif); font-size:1.2rem; color:var(--ink-1);",
                "Preparing your audiobook\u{2026}"
            }
            p {
                style: "margin-top:0.5rem; font-family:var(--mono); font-size:0.85rem; color:var(--ink-3);",
                "This may take a moment on first listen."
            }
        }
    }
}
