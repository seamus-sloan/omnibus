//! Cross-format resume prompt on the web player: when the linked book's
//! reading position is newer, a floating dismissable card offers the
//! mapped jump. Declining stores the source's event clock locally so the
//! prompt re-arms only after the reading position advances — never a
//! progress write. Web-only surface, like the mini dock.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::cross_format::{CrossFormatCandidate, CrossFormatResumeState};

use crate::components::alignment_modal::fmt_hm;
use crate::routes::Route;

/// Dismissal memory: the newest source clock the user has declined, per
/// `(book, target)`. Stored in localStorage on web; no-ops elsewhere.
pub(crate) fn dismissed_clock(uuid: &str, target: &str) -> Option<i64> {
    #[cfg(feature = "web")]
    {
        let storage = web_sys::window()?.local_storage().ok()??;
        storage
            .get_item(&format!("omn.syncprompt::{uuid}::{target}"))
            .ok()?
            .and_then(|v| v.parse().ok())
    }
    #[cfg(not(feature = "web"))]
    {
        let _ = (uuid, target);
        None
    }
}

/// Persist a declined candidate's source clock (see [`dismissed_clock`]).
pub(crate) fn store_dismissed_clock(uuid: &str, target: &str, clock: i64) {
    #[cfg(feature = "web")]
    {
        if let Some(storage) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
            let _ = storage.set_item(
                &format!("omn.syncprompt::{uuid}::{target}"),
                &clock.to_string(),
            );
        }
    }
    #[cfg(not(feature = "web"))]
    {
        let _ = (uuid, target, clock);
    }
}

/// Fetch the full cross-format resume for `(uuid, target)` — raw REST
/// over the same-origin cookie session (the `fetch_manifest` pattern).
/// `None` on any transport failure: the surfaces this feeds are
/// best-effort chrome, never an error surface.
pub(crate) async fn fetch_resume(
    uuid: &str,
    target: &str,
) -> Option<omnibus_shared::cross_format::CrossFormatResume> {
    #[cfg(feature = "web")]
    {
        let url = format!("/api/books/{uuid}/cross-format-resume?target={target}");
        let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
        if resp.status() != 200 {
            return None;
        }
        resp.json().await.ok()
    }
    #[cfg(not(feature = "web"))]
    {
        let _ = (uuid, target);
        None
    }
}

/// The floating prompt card. Renders nothing until the post-mount fetch
/// finds an undismissed candidate (SSR and first WASM paint agree).
#[component]
pub(super) fn SyncJumpPrompt(uuid: String) -> Element {
    let mut candidate = use_signal(|| None::<CrossFormatCandidate>);
    let mut hidden = use_signal(|| false);
    let playback = crate::use_playback();

    let fetch_uuid = uuid.clone();
    use_effect(move || {
        let uuid = fetch_uuid.clone();
        spawn(async move {
            let Some(resume) = fetch_resume(&uuid, "audio").await else {
                return;
            };
            if resume.state != CrossFormatResumeState::Candidate {
                return;
            }
            let Some(c) = resume.candidate else {
                return;
            };
            if resume.follow {
                // Follow mode: resolve-at-open. Apply the mapped position
                // silently — same seek-vs-navigate file guard as an
                // accepted offer, no card, no dismissal bookkeeping.
                let seconds = c.audio_position_seconds.unwrap_or(0.0);
                let loaded = (playback.file_id)();
                let same_file = loaded == c.book_file_id
                    || (loaded.is_none()
                        && ((playback.duration)() - c.total_duration_seconds.unwrap_or(0.0)).abs()
                            < 1.0);
                if same_file {
                    super::helpers::seek_to(seconds);
                } else {
                    dioxus_router::navigator().push(Route::BookListen {
                        uuid: uuid.clone(),
                        file_id: c.book_file_id,
                    });
                }
                return;
            }
            // Re-arm rule: an already-declined source position stays quiet
            // until the reading position advances past it.
            if dismissed_clock(&uuid, "audio")
                .is_some_and(|seen| c.source_client_updated_at <= seen)
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
    let seconds = c.audio_position_seconds.unwrap_or(0.0);
    let label = fmt_hm(seconds);
    let dismiss_uuid = uuid.clone();
    let accept_uuid = uuid.clone();
    let source_clock = c.source_client_updated_at;
    let target_file = c.book_file_id;
    let total = c.total_duration_seconds.unwrap_or(0.0);

    // A backward offer must not claim "further".
    let copy = if c.source_ahead == Some(false) {
        format!("You're re-reading earlier in the ebook — jump back to \u{2248} {label}?")
    } else {
        format!("You read further in the ebook — jump to \u{2248} {label}?")
    };

    rsx! {
        div { class: "lp-sync-prompt card", role: "status", "data-testid": "sync-prompt",
            span { class: "lp-sync-copy", "{copy}" }
            button {
                class: "btn sm",
                "data-testid": "sync-prompt-accept",
                onclick: move |_| {
                    hidden.set(true);
                    store_dismissed_clock(&accept_uuid, "audio", source_clock);
                    // Seek only when the mapped file is the one the player
                    // loaded (explicitly, or implicitly for a single-file
                    // timeline); otherwise navigate so the right file
                    // loads and this prompt re-offers the precise seek.
                    let loaded = (playback.file_id)();
                    let same_file = loaded == target_file
                        || (loaded.is_none() && ((playback.duration)() - total).abs() < 1.0);
                    if same_file {
                        super::helpers::seek_to(seconds);
                    } else {
                        dioxus_router::navigator().push(Route::BookListen {
                            uuid: accept_uuid.clone(),
                            file_id: target_file,
                        });
                    }
                },
                "Jump"
            }
            button {
                class: "btn ghost sm",
                "data-testid": "sync-prompt-dismiss",
                onclick: move |_| {
                    store_dismissed_clock(&dismiss_uuid, "audio", source_clock);
                    hidden.set(true);
                },
                "Keep my spot"
            }
            Link {
                to: Route::BookDetail { uuid: uuid.clone() },
                class: "lp-sync-alignment",
                "data-testid": "sync-prompt-alignment",
                "View alignment \u{2192}"
            }
        }
    }
}

/// "Synced here" toolbar control: declares the current listening position
/// as a sync point and turns follow mode on. Configuration-shaped (rule
/// 08): direct call, never queued, failure shown in place.
#[component]
pub(super) fn SyncHereButton() -> Element {
    let playback = crate::use_playback();
    let mut label = use_signal(|| "Synced here");
    // Seeded online for SSR parity (rule 07); reconciled post-mount.
    let mut online = use_signal(|| true);
    use_effect(move || online.set(browser_online()));
    rsx! {
        button {
            class: "btn sm lp-toolbar-btn",
            r#type: "button",
            "data-testid": "listen-sync-here",
            title: "Declare the ebook and audiobook aligned at this spot",
            disabled: !online(),
            onclick: move |_| {
                if !browser_online() {
                    return;
                }
                let Some(uuid) = (playback.uuid)() else { return };
                let decl = omnibus_shared::cross_format::DeclareSyncPoint {
                    book_uuid: uuid,
                    format: omnibus_shared::ProgressFormat::Audio,
                    ebook_fraction: None,
                    audio_book_file_id: (playback.file_id)(),
                    audio_seconds: Some((playback.elapsed)()),
                };
                spawn(async move {
                    label.set("Syncing\u{2026}");
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

/// Whether the browser reports connectivity — the "synced here" controls
/// disable offline (rule 08). Always `true` off-web.
pub(crate) fn browser_online() -> bool {
    #[cfg(feature = "web")]
    {
        web_sys::window()
            .map(|w| w.navigator().on_line())
            .unwrap_or(true)
    }
    #[cfg(not(feature = "web"))]
    {
        true
    }
}
