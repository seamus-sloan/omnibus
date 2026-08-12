//! Cross-format sync entry panel on the book detail page (web): the
//! unlinked nudge, the linked chip, and the stale re-confirm warning —
//! each opening the alignment modal. SSR and the first WASM paint render
//! the same neutral shell; a post-mount effect fills the state (rule 07).

use dioxus::prelude::*;
use omnibus_shared::AlignmentLink;

use crate::components::alignment_modal::AlignmentModal;
use crate::data;

/// The sync entry row + its modal. Mounted only for dual-format books.
/// `refresh` re-fetches on the page's own reload signal; `after_merge`
/// auto-opens the modal once when a merge just produced this dual-format
/// book (the merge dialog's second entry point).
#[component]
pub(super) fn BdSyncPanel(
    uuid: String,
    refresh: Signal<u32>,
    after_merge: Signal<bool>,
) -> Element {
    // Outer None = state unknown (SSR / first paint); inner Option is the
    // fetched link.
    let mut link = use_signal(|| None::<Option<AlignmentLink>>);
    let modal_open = use_signal(|| false);
    let mut epoch = use_signal(|| 0u32);

    let fetch_uuid = uuid.clone();
    use_effect(move || {
        let _ = refresh();
        let _ = epoch();
        let uuid = fetch_uuid.clone();
        spawn(async move {
            if let Ok(v) = data::get_alignment("", &uuid).await {
                link.set(Some(v.link));
            }
        });
    });

    {
        let mut after_merge = after_merge;
        let mut modal_open = modal_open;
        use_effect(move || {
            if after_merge() {
                after_merge.set(false);
                modal_open.set(true);
            }
        });
    }

    let mut open_modal = modal_open;
    rsx! {
        section { class: "bd-sync-panel", "data-testid": "sync-link-row",
            match link() {
                None => rsx! {},
                Some(None) => rsx! {
                    div { class: "al-entry al-entry-unlinked",
                        span { class: "al-entry-copy",
                            "Positions aren't synced — each format keeps its own spot."
                        }
                        button {
                            class: "btn sm",
                            "data-testid": "sync-link-open",
                            onclick: move |_| open_modal.set(true),
                            "Link formats…"
                        }
                    }
                },
                Some(Some(l)) if l.stale => rsx! {
                    div { class: "al-entry al-entry-stale", role: "status",
                        span { class: "al-entry-copy",
                            "The audiobook files changed since you linked — sync is paused "
                            "until you re-confirm the alignment."
                        }
                        button {
                            class: "btn sm",
                            "data-testid": "sync-link-review",
                            onclick: move |_| open_modal.set(true),
                            "Review alignment"
                        }
                    }
                },
                Some(Some(_)) => rsx! {
                    button {
                        class: "al-entry al-entry-linked",
                        "data-testid": "sync-link-manage",
                        onclick: move |_| open_modal.set(true),
                        span { class: "al-entry-copy", "Positions synced" }
                        span { class: "al-entry-meta", "ebook \u{2194} audiobook" }
                        span { class: "al-entry-chevron", "\u{203a}" }
                    }
                },
            }
            AlignmentModal {
                uuid: uuid.clone(),
                open: modal_open,
                on_changed: move |_| epoch.set(epoch() + 1),
            }
        }
    }
}
