//! Cross-format resume banner on the web reader: when the linked book's
//! listening position is newer, a slim banner under the top bar offers the
//! mapped percent jump (via the glue's `displayPercentage`). Declining
//! stores the source clock locally so the banner re-arms only after the
//! listening position advances. Web-only surface.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::cross_format::CrossFormatCandidate;

use crate::pages::listen::sync_prompt::{dismissed_clock, fetch_candidate, store_dismissed_clock};

/// The banner. Renders nothing until the post-mount fetch finds an
/// undismissed candidate, so SSR and the first WASM paint agree.
#[component]
pub(super) fn SyncJumpBanner(uuid: String) -> Element {
    let mut candidate = use_signal(|| None::<CrossFormatCandidate>);
    let mut hidden = use_signal(|| false);

    let fetch_uuid = uuid.clone();
    use_effect(move || {
        let uuid = fetch_uuid.clone();
        spawn(async move {
            let Some(c) = fetch_candidate(&uuid, "epub").await else {
                return;
            };
            if dismissed_clock(&uuid, "epub").is_some_and(|seen| c.source_client_updated_at <= seen)
            {
                return;
            }
            candidate.set(Some(c));
        });
    });

    let Some(c) = candidate() else {
        return rsx! {};
    };
    if hidden() {
        return rsx! {};
    }
    let Some(pct) = c.percent else {
        return rsx! {};
    };
    // Jump at full precision (the floored `pct` is display copy only) —
    // integer percent alone lands up to a page early on a long book.
    let jump_pct = c
        .fraction
        .map(|f| (f * 100.0).clamp(0.0, 100.0))
        .unwrap_or(pct as f64);
    let source_clock = c.source_client_updated_at;
    let jump_uuid = uuid.clone();
    let dismiss_uuid = uuid.clone();

    // A surviving backward offer (the listener deliberately went back)
    // must not claim "past this page" — say where the jump actually goes.
    let behind = c.source_ahead == Some(false);
    let copy = if behind {
        format!("Your audiobook sits earlier — jump back to \u{2248} {pct}%?")
    } else {
        format!("You listened past this page — jump to \u{2248} {pct}%?")
    };
    let jump_label = if behind { "Jump back" } else { "Jump" };

    rsx! {
        div { class: "rd-sync-banner card", role: "status", "data-testid": "sync-banner",
            span { class: "rd-sync-copy", "{copy}" }
            button {
                class: "btn sm",
                "data-testid": "sync-banner-jump",
                onclick: move |_| {
                    store_dismissed_clock(&jump_uuid, "epub", source_clock);
                    hidden.set(true);
                    // The glue only exists on interactive targets; SSR
                    // compiles this handler but never runs it.
                    #[cfg(feature = "web")]
                    super::reader_call("displayPercentage", &jump_pct.to_string());
                    #[cfg(not(feature = "web"))]
                    let _ = jump_pct;
                },
                "{jump_label}"
            }
            button {
                class: "btn ghost sm",
                "data-testid": "sync-banner-dismiss",
                onclick: move |_| {
                    store_dismissed_clock(&dismiss_uuid, "epub", source_clock);
                    hidden.set(true);
                },
                "Stay here"
            }
        }
    }
}
