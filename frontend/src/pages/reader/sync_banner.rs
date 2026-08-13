//! Cross-format resume banner on the web reader: when the linked book's
//! listening position is newer, a slim banner under the top bar offers the
//! mapped percent jump (via the glue's `displayPercentage`). Declining
//! stores the source clock locally so the banner re-arms only after the
//! listening position advances. Web-only surface.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::cross_format::CrossFormatCandidate;

use crate::pages::listen::sync_prompt::{dismissed_clock, store_dismissed_clock};

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
            let Some(resume) = crate::pages::listen::sync_prompt::fetch_resume(&uuid, "epub").await
            else {
                return;
            };
            if resume.state != omnibus_shared::cross_format::CrossFormatResumeState::Candidate {
                return;
            }
            let Some(c) = resume.candidate else {
                return;
            };
            if resume.follow {
                // Follow mode: resolve-at-open — move the text to the
                // mapped spot silently, full precision, no banner.
                #[cfg(feature = "web")]
                {
                    let jump = c
                        .fraction
                        .map(|f| (f * 100.0).clamp(0.0, 100.0))
                        .or(c.percent.map(|p| p as f64));
                    if let Some(pct) = jump {
                        super::reader_call("displayPercentage", &pct.to_string());
                    }
                }
                return;
            }
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

    // A backward offer must not claim "past this page". The listening
    // position + recency come along per the design ("≈ 16h 05m, yesterday").
    let behind = c.source_ahead == Some(false);
    let listened = c
        .source_position_seconds
        .map(|s| {
            let when = crate::components::alignment_modal::recency(c.source_client_updated_at);
            if when.is_empty() {
                format!(
                    " (\u{2248} {})",
                    crate::components::alignment_modal::fmt_hm(s)
                )
            } else {
                format!(
                    " (\u{2248} {}, {when})",
                    crate::components::alignment_modal::fmt_hm(s)
                )
            }
        })
        .unwrap_or_default();
    let copy = if behind {
        format!("Your audiobook sits earlier{listened} — jump back to \u{2248} {pct}%?")
    } else {
        format!("You listened past this page{listened} — jump to \u{2248} {pct}%?")
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
            dioxus_router::Link {
                to: crate::routes::Route::BookDetail { uuid: uuid.clone() },
                class: "lp-sync-alignment",
                "data-testid": "sync-banner-alignment",
                "View alignment \u{2192}"
            }
        }
    }
}

/// "Synced here" footer pill: declares the current reading position (full
/// precision) as a sync point and turns follow mode on. Configuration-
/// shaped (rule 08): direct call, never queued, failure shown in place.
#[component]
pub(super) fn SyncHerePill(uuid: String, loc: Signal<super::signals::RelocateData>) -> Element {
    let mut label = use_signal(|| "Synced here");
    // Seeded online for SSR parity (rule 07); reconciled post-mount.
    let mut online = use_signal(|| true);
    use_effect(move || online.set(crate::pages::listen::sync_prompt::browser_online()));
    rsx! {
        button {
            class: "btn ghost sm rd-sync-here",
            r#type: "button",
            "data-testid": "reader-sync-here",
            title: "Declare the ebook and audiobook aligned at this spot",
            disabled: !online(),
            onclick: move |_| {
                if !crate::pages::listen::sync_prompt::browser_online() {
                    return;
                }
                let uuid = uuid.clone();
                let frac = loc.peek().frac;
                spawn(async move {
                    label.set("Syncing\u{2026}");
                    let decl = omnibus_shared::cross_format::DeclareSyncPoint {
                        book_uuid: uuid,
                        format: omnibus_shared::ProgressFormat::Epub,
                        ebook_fraction: Some(frac.clamp(0.0, 1.0)),
                        audio_book_file_id: None,
                        audio_seconds: None,
                    };
                    match crate::data::declare_sync_point("", decl).await {
                        Ok(()) => label.set("Synced \u{2713}"),
                        Err(_) => label.set("Sync failed"),
                    }
                });
            },
            "{label}"
        }
    }
}
