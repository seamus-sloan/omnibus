//! Public reading-journal section for the book-detail page. Renders every
//! reader's entries plus an owner-only composer with a live markdown editor
//! (see `journal_editor.js`). Entries, the current user, and the write surface
//! all attach post-mount so SSR and first-hydration paint stay identical
//! (rule 07); bodies are sanitized server-side. The draft composer lives in
//! [`composer`] and the per-entry card (plus its inline editor) in
//! [`entry_card`] — this file keeps the page-level listing/orchestration.

use dioxus::prelude::*;
use omnibus_shared::JournalEntry;

use crate::{data, use_server_url};

use super::BdSectionHead;

mod composer;
mod entry_card;

use composer::BdJournalComposer;
use entry_card::BdJournalEntryCard;

/// Public reading-journal section: header, composer, and the entry feed.
#[component]
pub(super) fn BdJournalSection(uuid: String) -> Element {
    let server_url = use_server_url();
    let entries = use_signal(Vec::<JournalEntry>::new);
    // Derived from the app-wide `CurrentUser` context (`crate::use_current_user_summary`)
    // instead of an independent per-mount `/api/auth/me` fetch. Mobile/SSR
    // stay at the `None` default since the context is web-only.
    let current_user = crate::use_current_user_summary();
    // Bumped after any mutation to refetch the server-authoritative feed.
    let reload = use_signal(|| 0u32);

    use_journal_entries_load(uuid.clone(), server_url.clone(), reload, entries);
    use_spoiler_reveal_binding();

    let list = entries();
    let kicker = journal_kicker(&list);

    rsx! {
        div { id: "journal", class: "bd-journal", "data-testid": "journal-section",
            BdSectionHead { kicker, title: "What readers have written".to_string() }
            p { class: "mono bd-journal-blurb",
                "A shared log — every reader's journal for this book lives here."
            }
            BdJournalComposer { uuid: uuid.clone(), server_url: server_url.clone(), reload }
            BdJournalList { list, current_user: current_user(), server_url, reload }
        }
    }
}

/// Loads the journal feed for `uuid` on mount, whenever `uuid` changes, and
/// whenever `reload` bumps (after a create/edit/delete). Called
/// unconditionally from [`BdJournalSection`].
fn use_journal_entries_load(
    uuid: String,
    load_url: String,
    reload: Signal<u32>,
    mut entries: Signal<Vec<JournalEntry>>,
) {
    use_effect(use_reactive!(|uuid| {
        let _ = reload();
        let url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            if let Ok(list) = data::list_journal_entries(&url, &uuid).await {
                entries.set(list);
            }
        });
    }));
}

/// Binds click-to-reveal spoilers, delegated on `document` so it covers
/// entries that render after mount and bound once via a window guard. The
/// sanitizer emits the spoiler as a real `<button>`, so Tab reaches it and
/// Enter/Space fire a native click without a keydown listener; the handler
/// just needs to keep `aria-expanded` in sync with the `.revealed` class so
/// assistive tech reflects the toggled state. Web-only: the eval is a no-op
/// on SSR / native, and gating the body (not the hook) keeps the hook count
/// identical across targets for hydration. Called unconditionally from
/// [`BdJournalSection`].
fn use_spoiler_reveal_binding() {
    use_effect(move || {
        #[cfg(feature = "web")]
        {
            let _ = dioxus::document::eval(
                r#"
                if (!window.__omnibusSpoilerBound) {
                    window.__omnibusSpoilerBound = true;
                    document.addEventListener('click', (e) => {
                        const s = e.target.closest && e.target.closest('.spoiler');
                        if (!s) return;
                        const revealed = s.classList.toggle('revealed');
                        s.setAttribute('aria-expanded', revealed ? 'true' : 'false');
                    });
                }
                "#,
            );
        }
    });
}

/// The section-head kicker text: entry/reader counts, or an empty-state note.
fn journal_kicker(list: &[JournalEntry]) -> String {
    if list.is_empty() {
        return "Reading journal · no entries yet".to_string();
    }
    let reader_count = {
        let mut ids: Vec<i64> = list.iter().map(|e| e.author_id).collect();
        ids.sort_unstable();
        ids.dedup();
        ids.len()
    };
    let entry_word = if list.len() == 1 { "entry" } else { "entries" };
    let reader_word = if reader_count == 1 {
        "reader"
    } else {
        "readers"
    };
    format!(
        "Reading journal · {} {entry_word} from {reader_count} {reader_word}",
        list.len()
    )
}

/// The empty-state card, or the feed of entry cards.
#[component]
fn BdJournalList(
    list: Vec<JournalEntry>,
    current_user: Option<omnibus_shared::UserSummary>,
    server_url: String,
    reload: Signal<u32>,
) -> Element {
    rsx! {
        if list.is_empty() {
            div { class: "bd-journal-empty card", "data-testid": "journal-empty",
                p { class: "mono", "No journal entries yet — be the first to write one." }
            }
        } else {
            div { class: "bd-journal-list", "data-testid": "journal-list",
                for entry in list.iter() {
                    BdJournalEntryCard {
                        key: "{entry.id}",
                        entry: entry.clone(),
                        current_user: current_user.clone(),
                        server_url: server_url.clone(),
                        reload,
                    }
                }
            }
        }
    }
}
