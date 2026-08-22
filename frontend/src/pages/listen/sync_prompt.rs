//! Cross-format transport for the web player: the resume fetch shared by
//! the reader's follow resolver and the listen bootstrap, plus the
//! "Synced here" toolbar control. Follow-only — the old "jump ahead?"
//! card is gone (linking IS turning sync on), and follow resolution at
//! boot owns the mapped position. Web-only surface, like the mini dock.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;

/// Fetch the full cross-format resume for `(uuid, target)` — raw REST
/// over the same-origin cookie session. `derive_cfi` asks the server to
/// attach the mapped point CFI (`?derive=cfi`) — only the
/// reader surfaces ask (`derive=cfi` opens the EPUB server-side), so the
/// audio/boot callers stay on the plain fetch.
pub(crate) async fn fetch_resume_with(
    uuid: &str,
    target: &str,
    derive_cfi: bool,
) -> Option<omnibus_shared::cross_format::CrossFormatResume> {
    #[cfg(feature = "web")]
    {
        let derive = if derive_cfi { "&derive=cfi" } else { "" };
        let url = format!("/api/books/{uuid}/cross-format-resume?target={target}{derive}");
        let resp = gloo_net::http::Request::get(&url).send().await.ok()?;
        if resp.status() != 200 {
            return None;
        }
        resp.json().await.ok()
    }
    #[cfg(not(feature = "web"))]
    {
        let _ = (uuid, target, derive_cfi);
        None
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
                    epub_cfi: None,
                    // The RESOLVED file, not the picker request: `file_id`
                    // is `None` on every ordinary resume path, and a
                    // file-less audio declaration on a multi-file timeline
                    // is (correctly) rejected server-side.
                    audio_book_file_id: (playback.loaded_file_id)(),
                    audio_seconds: Some((playback.elapsed)()),
                };
                spawn(async move {
                    label.set("Syncing\u{2026}");
                    label.set(crate::data::declare_sync_and_label(decl).await);
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
