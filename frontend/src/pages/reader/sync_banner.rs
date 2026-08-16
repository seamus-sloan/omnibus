//! Cross-format follow resolution on the web reader, plus the "Synced
//! here" footer pill. Follow-only: opening a linked book whose listening
//! position is newer silently moves the text to the mapped spot (the
//! server-derived CFI when available). The old "jump ahead?" banner is
//! gone — linking IS turning sync on, and a linked book that still
//! prompts on every surface switch reads as broken. Web-only surface.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Invisible resolver: fetches the resume candidate once per mount and,
/// in follow mode, applies the mapped position through the glue. Renders
/// nothing on every target, so SSR and the first WASM paint agree.
#[component]
pub(super) fn SyncJumpBanner(uuid: String) -> Element {
    let fetch_uuid = uuid.clone();
    use_effect(move || {
        let uuid = fetch_uuid.clone();
        spawn(async move {
            let Some(resume) =
                crate::pages::listen::sync_prompt::fetch_resume_with(&uuid, "epub", true).await
            else {
                return;
            };
            if resume.state != omnibus_shared::cross_format::CrossFormatResumeState::Candidate {
                return;
            }
            let Some(c) = resume.candidate else {
                return;
            };
            if !resume.follow {
                return;
            }
            // Only the web target has a glue to drive; SSR compiles the
            // candidate binding but never consumes it.
            #[cfg(not(feature = "web"))]
            let _ = c;
            // Resolve-at-open — move the text to the mapped spot silently.
            // The server-derived CFI is the precise target (one ruler end
            // to end); the locations-scale percentage is only the fallback.
            #[cfg(feature = "web")]
            {
                if let Some(cfi) = c.epub_cfi.as_deref() {
                    super::reader_call_json("displayCfi", cfi);
                } else if let Some(pct) = c
                    .fraction
                    .map(|f| (f * 100.0).clamp(0.0, 100.0))
                    .or(c.percent.map(|p| p as f64))
                {
                    super::reader_call("displayPercentage", &pct.to_string());
                }
            }
        });
    });
    rsx! {}
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
    // An anchor is only as good as the fraction it records: until the
    // locations pass resolves, `frac` is the rounded spine approximation
    // (and before the first relocate it's the 0.0 default with no CFI) —
    // declaring from either would bend every later mapping around a wrong
    // "ground truth" pair. Reactive read: the pill enables itself the
    // moment a precise relocate lands.
    let precise = {
        let l = loc.read();
        !l.pct_approx && l.cfi.is_some()
    };
    rsx! {
        button {
            class: "btn ghost sm rd-sync-here",
            r#type: "button",
            "data-testid": "reader-sync-here",
            title: "Declare the ebook and audiobook aligned at this spot",
            disabled: !online() || !precise,
            onclick: move |_| {
                if !crate::pages::listen::sync_prompt::browser_online() {
                    return;
                }
                let uuid = uuid.clone();
                let frac = loc.peek().frac;
                // The CFI names this spot on the server's own ruler; the
                // locations-scale fraction rides along as the fallback.
                let cfi = loc.peek().cfi.clone();
                spawn(async move {
                    label.set("Syncing\u{2026}");
                    let decl = omnibus_shared::cross_format::DeclareSyncPoint {
                        book_uuid: uuid,
                        format: omnibus_shared::ProgressFormat::Epub,
                        ebook_fraction: Some(frac.clamp(0.0, 1.0)),
                        epub_cfi: cfi,
                        audio_book_file_id: None,
                        audio_seconds: None,
                    };
                    match crate::data::declare_sync_point("", decl).await {
                        Ok(()) => label.set("Synced \u{2713}"),
                        Err(crate::data::SyncPointError::LinkRequired) => {
                            label.set("Link formats first")
                        }
                        Err(_) => label.set("Sync failed"),
                    }
                });
            },
            "{label}"
        }
    }
}
