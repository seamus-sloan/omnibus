//! Public reading-journal section for the book-detail page: every reader's
//! entries plus an owner-only composer with a live markdown editor. Entries,
//! the current user, and the write surface all attach post-mount so SSR and
//! first-hydration paint stay identical; bodies are sanitized server-side. The
//! composer lives in [`composer`] and the per-entry card in [`entry_card`].

use dioxus::prelude::*;
use omnibus_shared::JournalEntry;

#[cfg(not(feature = "mobile"))]
use crate::components::user_avatar::UserAvatar;
use crate::{data, use_server_url};

use super::dates::use_local_dates_ready;
#[cfg(feature = "mobile")]
use super::BdSectionHead;

mod composer;
mod entry_card;

use composer::BdJournalComposer;
use entry_card::BdJournalEntryCard;

/// Public reading-journal section: header, composer, and the entry feed.
/// Mobile's single-column re-flow; the web stage uses [`W4JournalStop`].
#[cfg(feature = "mobile")]
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
#[cfg(feature = "mobile")]
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

/// Stop 05 · Journals for the W4 stage: header (entry/reader counts + the
/// reader avatar stack), the composer, and the excerpt ladder — one line per
/// entry, tap to open the full entry card over the stop. Wishlist-only books
/// render the design's empty state instead.
#[cfg(not(feature = "mobile"))]
#[component]
pub(super) fn W4JournalStop(uuid: String, wish_mode: bool) -> Element {
    let server_url = use_server_url();
    let entries = use_signal(Vec::<JournalEntry>::new);
    let current_user = crate::use_current_user_summary();
    let reload = use_signal(|| 0u32);
    // The entry opened in the overlay, by id (not index — a reload after an
    // edit must keep the same entry open or drop it if it went away).
    let mut open_entry = use_signal(|| None::<i64>);

    use_journal_entries_load(uuid.clone(), server_url.clone(), reload, entries);
    use_spoiler_reveal_binding();
    let dates_ready = use_local_dates_ready();

    let list = entries();
    let published = list
        .iter()
        .filter(|e| e.status != omnibus_shared::JournalStatus::Draft)
        .count();
    let drafts = list.len() - published;
    let readers: Vec<&JournalEntry> = {
        let mut seen = std::collections::HashSet::new();
        list.iter().filter(|e| seen.insert(e.author_id)).collect()
    };
    let kicker = w4_journal_kicker(published, readers.len(), drafts, wish_mode);
    let opened = open_entry().and_then(|id| list.iter().find(|e| e.id == id).cloned());

    rsx! {
        div { id: "journal", class: "bd-journal bdw4-journal", "data-testid": "journal-section",
        div { class: "bdw4-journalhead",
            div { class: "bdw4-k", "{kicker}" }
            if readers.len() > 1 {
                div { class: "bdw4-avatarstack", aria_hidden: "true",
                    for e in readers.iter().take(5) {
                        UserAvatar {
                            key: "{e.author_id}",
                            user_id: e.author_id,
                            name: e.author_name.clone(),
                            has_avatar: e.author_has_avatar,
                            class: "bdw4-stack-avatar".to_string(),
                        }
                    }
                }
            }
        }
        if wish_mode {
            div { class: "bdw4-bigquiet", "No entries yet \u{2014} the journal begins when you do." }
            p { class: "mono bdw4-quiet-hint", "a shared log \u{2014} anyone reading this book can write here" }
        } else {
            BdJournalComposer { uuid: uuid.clone(), server_url: server_url.clone(), reload }
            if list.is_empty() {
                div { class: "bd-journal-empty card", "data-testid": "journal-empty",
                    p { class: "mono", "No journal entries yet \u{2014} be the first to write one." }
                }
            } else {
                p { class: "mono bdw4-ladderhint", aria_hidden: "true",
                    "one line each \u{2014} tap to open the full entry"
                }
                div { class: "bdw4-ladder", "data-testid": "journal-list",
                    for entry in list.iter() {
                        {render_ladder_row(entry, open_entry)}
                    }
                }
            }
        }
        if let Some(entry) = opened {
            div {
                class: "bdw4-overlay",
                "data-testid": "journal-overlay",
                onclick: move |_| open_entry.set(None),
                div {
                    class: "bdw4-ocard",
                    onclick: move |e| e.stop_propagation(),
                    BdJournalEntryCard {
                        entry,
                        current_user: current_user(),
                        server_url: server_url.clone(),
                        reload,
                        dates_ready,
                    }
                    button {
                        class: "btn ghost sm bdw4-ocard-close",
                        "data-testid": "journal-overlay-close",
                        onclick: move |_| open_entry.set(None),
                        "\u{2715} Close"
                    }
                }
            }
        }
        }
    }
}

/// One excerpt-ladder row: avatar · first name · (draft chip) · first line ·
/// progress percent.
#[cfg(not(feature = "mobile"))]
fn render_ladder_row(entry: &JournalEntry, mut open_entry: Signal<Option<i64>>) -> Element {
    let id = entry.id;
    let is_draft = entry.status == omnibus_shared::JournalStatus::Draft;
    let first_name = entry
        .author_name
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_string();
    let excerpt = journal_excerpt(&entry.body_md);
    rsx! {
        button {
            key: "{id}",
            r#type: "button",
            class: if is_draft { "bdw4-lrow draft" } else { "bdw4-lrow" },
            "data-testid": "journal-ladder-row",
            onclick: move |_| open_entry.set(Some(id)),
            UserAvatar {
                user_id: entry.author_id,
                name: entry.author_name.clone(),
                has_avatar: entry.author_has_avatar,
                class: "bdw4-lrow-avatar".to_string(),
            }
            span { class: "nm", "{first_name}" }
            if is_draft {
                span { class: "bdw4-draftchip", "data-testid": "journal-draft-chip-row", "Draft" }
            }
            span { class: "ex", "{excerpt}" }
            if let Some(p) = entry.progress {
                span { class: "at", "{p}%" }
            }
        }
    }
}

/// The W4 journal kicker: `The journal · N entries from M readers · d draft`.
#[cfg(not(feature = "mobile"))]
fn w4_journal_kicker(published: usize, readers: usize, drafts: usize, wish_mode: bool) -> String {
    if wish_mode {
        return "The journal \u{b7} empty".to_string();
    }
    if published + drafts == 0 {
        return "The journal \u{b7} no entries yet".to_string();
    }
    let entry_word = if published == 1 { "entry" } else { "entries" };
    let reader_word = if readers == 1 { "reader" } else { "readers" };
    let mut out =
        format!("The journal \u{b7} {published} {entry_word} from {readers} {reader_word}");
    if drafts > 0 {
        out.push_str(&format!(
            " \u{b7} {drafts} {}",
            if drafts == 1 { "draft" } else { "drafts" }
        ));
    }
    out
}

/// First non-empty line of a markdown body, markup stripped and spoilers
/// masked, capped for the one-line ladder row.
#[cfg(not(feature = "mobile"))]
fn journal_excerpt(md: &str) -> String {
    let line = md
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    // `||spoiler||` spans must not leak into the always-visible excerpt —
    // mask each span before stripping the remaining markers.
    let mut masked = String::with_capacity(line.len());
    for (i, seg) in line.split("||").enumerate() {
        masked.push_str(if i % 2 == 0 {
            seg
        } else {
            "\u{2588}\u{2588}\u{2588}"
        });
    }
    let stripped: String = masked
        .chars()
        .filter(|c| !matches!(c, '*' | '_' | '>' | '#' | '`'))
        .collect();
    let stripped = stripped.trim_start_matches("- ").trim().to_string();
    let mut out: String = stripped.chars().take(160).collect();
    if stripped.chars().count() > 160 {
        out.push('\u{2026}');
    }
    out
}

/// The empty-state card, or the feed of entry cards.
#[cfg(feature = "mobile")]
#[component]
fn BdJournalList(
    list: Vec<JournalEntry>,
    current_user: Option<omnibus_shared::UserSummary>,
    server_url: String,
    reload: Signal<u32>,
) -> Element {
    // Hoisted once for the whole feed rather than once per card — each card
    // resolves its own offset from its own `created_at` via
    // `dates::local_date_offset`, so sharing this readiness signal doesn't
    // share a stale offset across entries with different dates.
    let dates_ready = use_local_dates_ready();
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
                        dates_ready,
                    }
                }
            }
        }
    }
}
