//! Web-only physical-collection + wishlist panel for the book-detail page.
//!
//! Renders the "In your physical collection" pill with one card per checked-in
//! copy (edit-note / delete), the wishlist tracking card (Remove / Find a copy),
//! and the "Add to physical wishlist" affordance for any book without a copy.
//! Self-loads its data post-mount so SSR and the first WASM paint match
//! (rule 07); the page `refresh` signal is bumped after mutations that change
//! the book's physical/visibility state so the hero re-fetches.

use dioxus::prelude::*;
use omnibus_shared::physical::{PhysicalCopy, WishlistEntry, WishlistSource};

use crate::{data, use_server_url};

use super::BdSectionHead;

/// A pending copy deletion awaiting confirmation. `last_fileless` marks the
/// only copy of a book with no files — deleting it needs the remove-or-wishlist
/// choice instead of a plain confirm, since the book would otherwise vanish
/// from every library view (the visibility rule).
#[derive(Clone, Copy, PartialEq)]
struct DeleteTarget {
    copy_id: i64,
    last_fileless: bool,
}

/// The panel's mutable state, bundled so builder/mutation helpers take one
/// `Copy` value rather than a fistful of signals (mirrors `rating::RatingState`).
#[derive(Clone, Copy)]
struct PhysPanelState {
    copies: Signal<Vec<PhysicalCopy>>,
    wishlist: Signal<Option<WishlistEntry>>,
    busy: Signal<bool>,
    err: Signal<Option<String>>,
    editing: Signal<Option<i64>>,
    note_draft: Signal<String>,
    delete_target: Signal<Option<DeleteTarget>>,
    refresh: Signal<u32>,
}

/// Physical-collection + wishlist panel. `is_fileless` is `b.formats.is_empty()`
/// (no ebook/audiobook files); `isbn`/`title`/`author` feed the "Find a copy"
/// search. `refresh` is the page's book-refetch signal.
#[component]
pub(super) fn BdPhysicalPanel(
    uuid: String,
    is_fileless: bool,
    isbn: Option<String>,
    title: String,
    author: String,
    refresh: Signal<u32>,
) -> Element {
    let server_url = use_server_url();
    let user = crate::use_current_user_summary();
    let can_edit = user().map(|u| u.is_admin || u.can_edit).unwrap_or(false);

    let mut copies = use_signal(Vec::<PhysicalCopy>::new);
    let mut wishlist = use_signal(|| None::<WishlistEntry>);
    let mut loaded = use_signal(|| false);
    let state = PhysPanelState {
        copies,
        wishlist,
        busy: use_signal(|| false),
        err: use_signal(|| None::<String>),
        editing: use_signal(|| None::<i64>),
        note_draft: use_signal(String::new),
        delete_target: use_signal(|| None::<DeleteTarget>),
        refresh,
    };

    let load_url = server_url.clone();
    use_effect(use_reactive!(|uuid| {
        if uuid.is_empty() {
            return;
        }
        let load_url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            let c = data::list_physical_copies(&load_url, &uuid)
                .await
                .unwrap_or_default();
            let w = data::get_wishlist_entry(&load_url, &uuid)
                .await
                .unwrap_or(None);
            copies.set(c);
            wishlist.set(w);
            loaded.set(true);
        });
    }));

    // First paint (SSR + first WASM) renders nothing; the post-mount load fills
    // it in. No hooks past this point, so the early return keeps hook order
    // fixed across the empty and loaded renders (rule 07).
    if !loaded() {
        return rsx! {};
    }

    let content = if !copies().is_empty() {
        render_physical_section(
            state,
            server_url.clone(),
            uuid.clone(),
            is_fileless,
            can_edit,
        )
    } else if wishlist().is_some() {
        render_wishlist_section(state, server_url.clone(), uuid.clone(), isbn, title, author)
    } else {
        render_add_wishlist(state, server_url.clone(), uuid.clone())
    };

    rsx! {
        section { class: "bd-physical-panel", "data-testid": "bd-physical-panel",
            {content}
            if let Some(e) = state.err.read().clone() {
                p { role: "alert", class: "bd-phys-error", "data-testid": "physical-error", "{e}" }
            }
            {render_delete_modal(state, server_url, uuid)}
        }
    }
}

/// The checked-in-copies section: pill + one card per copy.
fn render_physical_section(
    state: PhysPanelState,
    url: String,
    uuid: String,
    is_fileless: bool,
    can_edit: bool,
) -> Element {
    let copies = state.copies;
    rsx! {
        BdSectionHead { kicker: "Physical", title: "Physical copy" }
        div { class: "bd-phys-pill-row",
            span { class: "chip bd-phys-pill", "data-testid": "physical-pill",
                "In your physical collection"
            }
        }
        div { class: "bd-phys-copies",
            for copy in copies() {
                {render_copy_card(state, url.clone(), uuid.clone(), copy, is_fileless, can_edit)}
            }
        }
    }
}

/// One physical-copy card: check-in date, ISBN, note (view or inline editor),
/// and the edit/delete actions (edit-gated).
fn render_copy_card(
    state: PhysPanelState,
    url: String,
    _uuid: String,
    copy: PhysicalCopy,
    is_fileless: bool,
    can_edit: bool,
) -> Element {
    let mut editing = state.editing;
    let mut note_draft = state.note_draft;
    let mut delete_target = state.delete_target;
    let copies_sig = state.copies;
    let copy_id = copy.id;
    let is_editing = editing() == Some(copy_id);
    let checked_in = checked_in_label(now_unix(), copy.checked_in_at);
    let note_val = copy.note.clone().unwrap_or_default();
    let note_is_empty = note_val.trim().is_empty();
    rsx! {
        div { class: "card bd-phys-copy", "data-testid": "physical-copy-card",
            div { class: "bd-phys-copy-head",
                span { class: "bd-phys-copy-date", "{checked_in}" }
                if let Some(isbn) = copy.isbn.clone() {
                    span { class: "mono bd-phys-copy-isbn", "ISBN {isbn}" }
                }
            }
            if is_editing {
                {render_note_editor(state, url, copy_id)}
            } else {
                if !note_is_empty {
                    p { class: "bd-phys-copy-note", "{note_val}" }
                }
                if can_edit {
                    div { class: "bd-phys-copy-actions",
                        button {
                            class: "btn ghost sm",
                            "data-testid": "copy-edit-note",
                            onclick: move |_| {
                                note_draft.set(note_val.clone());
                                editing.set(Some(copy_id));
                            },
                            if note_is_empty { "Add a note" } else { "Edit note" }
                        }
                        button {
                            class: "btn ghost sm bd-phys-danger",
                            "data-testid": "copy-delete",
                            onclick: move |_| {
                                let last_fileless = is_fileless && copies_sig.peek().len() == 1;
                                delete_target.set(Some(DeleteTarget { copy_id, last_fileless }));
                            },
                            "Remove\u{2026}"
                        }
                    }
                }
            }
        }
    }
}

/// Inline note editor for one copy: textarea + Save/Cancel.
fn render_note_editor(state: PhysPanelState, url: String, copy_id: i64) -> Element {
    let mut note_draft = state.note_draft;
    let mut editing = state.editing;
    let busy = state.busy;
    rsx! {
        div { class: "bd-phys-note-editor",
            label { class: "label", r#for: "copy-note-{copy_id}", "Edition note" }
            textarea {
                id: "copy-note-{copy_id}",
                class: "bd-phys-note-input",
                rows: 2,
                value: "{note_draft}",
                oninput: move |e| note_draft.set(e.value()),
            }
            div { class: "bd-phys-note-actions",
                button {
                    class: "btn sm",
                    "data-testid": "copy-note-save",
                    disabled: busy(),
                    onclick: move |_| {
                        let trimmed = note_draft.peek().trim().to_string();
                        let note = (!trimmed.is_empty()).then_some(trimmed);
                        save_note(state, url.clone(), copy_id, note);
                    },
                    "Save"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "copy-note-cancel",
                    onclick: move |_| editing.set(None),
                    "Cancel"
                }
            }
        }
    }
}

/// Wishlisted-book state: pill + tracking card with Remove and Find a copy.
fn render_wishlist_section(
    state: PhysPanelState,
    url: String,
    uuid: String,
    isbn: Option<String>,
    title: String,
    author: String,
) -> Element {
    let busy = state.busy;
    let entry = state.wishlist.read().clone();
    let added = entry
        .as_ref()
        .map(|e| time_ago(now_unix(), e.added_at))
        .unwrap_or_default();
    let source = entry.map(|e| source_label(e.source)).unwrap_or("a scan");
    let find_url = find_a_copy_url(isbn.as_deref(), &title, &author);
    rsx! {
        BdSectionHead { kicker: "Wishlist", title: "On your wishlist" }
        div { class: "bd-phys-pill-row",
            span { class: "chip bd-phys-pill bd-wishlist-pill", "data-testid": "wishlist-pill",
                "On your wishlist"
            }
        }
        div { class: "card bd-wishlist-card", "data-testid": "wishlist-card",
            p { class: "bd-wishlist-meta",
                "Tracking this title \u{00b7} added {added} from {source}"
            }
            div { class: "bd-wishlist-actions",
                a {
                    class: "btn sm",
                    "data-testid": "find-a-copy",
                    href: "{find_url}",
                    target: "_blank",
                    rel: "noopener noreferrer",
                    "Find a copy"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "wishlist-remove",
                    disabled: busy(),
                    onclick: move |_| remove_from_wishlist(state, url.clone(), uuid.clone()),
                    "Remove from wishlist"
                }
            }
        }
    }
}

/// The "Add to physical wishlist" affordance, shown for any book without a
/// physical copy — including one owned only digitally.
fn render_add_wishlist(state: PhysPanelState, url: String, uuid: String) -> Element {
    let busy = state.busy;
    rsx! {
        div { class: "card bd-wishlist-add", "data-testid": "wishlist-add-card",
            div { class: "bd-wishlist-add-text",
                div { class: "label bd-section-kicker", "Physical wishlist" }
                p { class: "bd-wishlist-add-blurb", "Track this title to find a physical copy later." }
            }
            button {
                class: "btn sm",
                "data-testid": "add-to-wishlist",
                disabled: busy(),
                onclick: move |_| add_to_wishlist(state, url.clone(), uuid.clone()),
                "Add to physical wishlist"
            }
        }
    }
}

/// Confirmation modal for a copy delete. A last-copy-on-fileless-book delete
/// gets the remove-or-wishlist choice; every other delete gets a plain "I sold
/// it" confirm.
fn render_delete_modal(state: PhysPanelState, url: String, uuid: String) -> Element {
    let mut delete_target = state.delete_target;
    let busy = state.busy;
    let Some(target) = delete_target() else {
        return rsx! {};
    };
    let copy_id = target.copy_id;
    if target.last_fileless {
        let (uw, ww) = (url.clone(), uuid.clone());
        rsx! {
            div {
                class: "author-photo-modal-backdrop",
                role: "dialog",
                aria_modal: "true",
                "data-testid": "last-copy-modal",
                onclick: move |_| if !busy() { delete_target.set(None); },
                div { class: "mg-modal del-modal", onclick: move |e| e.stop_propagation(),
                    h3 { class: "del-modal-title", "Remove your last copy" }
                    p { class: "del-modal-body",
                        "This is the only copy of a book with no files in your library. Remove it entirely, or keep tracking it on your wishlist?"
                    }
                    div { class: "del-modal-actions",
                        button {
                            class: "del-btn-ghost",
                            "data-testid": "last-copy-cancel",
                            disabled: busy(),
                            onclick: move |_| delete_target.set(None),
                            "Cancel"
                        }
                        button {
                            class: "del-btn-ghost",
                            "data-testid": "last-copy-wishlist",
                            disabled: busy(),
                            onclick: move |_| delete_last_and_wishlist(state, uw.clone(), ww.clone(), copy_id),
                            "Move to wishlist"
                        }
                        button {
                            class: "del-btn-danger",
                            "data-testid": "last-copy-remove",
                            disabled: busy(),
                            onclick: move |_| delete_last_and_remove(state, url.clone(), uuid.clone(), copy_id),
                            "Remove from library"
                        }
                    }
                }
            }
        }
    } else {
        rsx! {
            div {
                class: "author-photo-modal-backdrop",
                role: "dialog",
                aria_modal: "true",
                "data-testid": "copy-delete-modal",
                onclick: move |_| if !busy() { delete_target.set(None); },
                div { class: "mg-modal del-modal", onclick: move |e| e.stop_propagation(),
                    h3 { class: "del-modal-title", "Remove this copy?" }
                    p { class: "del-modal-body", "This removes the physical copy from your collection." }
                    div { class: "del-modal-actions",
                        button {
                            class: "del-btn-ghost",
                            "data-testid": "copy-delete-cancel",
                            disabled: busy(),
                            onclick: move |_| delete_target.set(None),
                            "Cancel"
                        }
                        button {
                            class: "del-btn-danger",
                            "data-testid": "copy-delete-confirm",
                            disabled: busy(),
                            onclick: move |_| delete_copy_only(state, url.clone(), copy_id),
                            "I sold it"
                        }
                    }
                }
            }
        }
    }
}

// Mutations. Each optimistically clears the error, marks busy, then reconciles.

/// Persist a copy's note and update it in the list on success.
fn save_note(state: PhysPanelState, url: String, copy_id: i64, note: Option<String>) {
    let PhysPanelState {
        mut copies,
        mut busy,
        mut err,
        mut editing,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        match data::update_physical_copy_note(&url, copy_id, note).await {
            Ok(updated) => {
                copies.with_mut(|list| {
                    if let Some(c) = list.iter_mut().find(|c| c.id == copy_id) {
                        *c = updated;
                    }
                });
                editing.set(None);
            }
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

/// Delete one copy (non-last, or a file-backed book). Bumps `refresh` so the
/// hero badge reflects the possibly-changed `has_physical`.
fn delete_copy_only(state: PhysPanelState, url: String, copy_id: i64) {
    let PhysPanelState {
        mut copies,
        mut busy,
        mut err,
        mut delete_target,
        mut refresh,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        match data::delete_physical_copy(&url, copy_id).await {
            Ok(()) => {
                copies.with_mut(|l| l.retain(|c| c.id != copy_id));
                delete_target.set(None);
                refresh.with_mut(|r| *r += 1);
            }
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

/// Last-copy "Remove from library": delete the copy, then the now-fileless
/// book. Bumps `refresh`; the book is gone, so the page re-renders not-found.
fn delete_last_and_remove(state: PhysPanelState, url: String, uuid: String, copy_id: i64) {
    let PhysPanelState {
        mut busy,
        mut err,
        mut delete_target,
        mut refresh,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        let result = async {
            data::delete_physical_copy(&url, copy_id).await?;
            data::delete_fileless_book(&url, &uuid).await
        }
        .await;
        match result {
            Ok(()) => {
                delete_target.set(None);
                refresh.with_mut(|r| *r += 1);
            }
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

/// Last-copy "Move to wishlist": delete the copy, then wishlist the book.
fn delete_last_and_wishlist(state: PhysPanelState, url: String, uuid: String, copy_id: i64) {
    let PhysPanelState {
        mut copies,
        mut wishlist,
        mut busy,
        mut err,
        mut delete_target,
        mut refresh,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        let result = async {
            data::delete_physical_copy(&url, copy_id).await?;
            data::add_wishlist_entry(&url, &uuid).await
        }
        .await;
        match result {
            Ok(entry) => {
                copies.with_mut(|l| l.retain(|c| c.id != copy_id));
                wishlist.set(Some(entry));
                delete_target.set(None);
                refresh.with_mut(|r| *r += 1);
            }
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

/// Add the book to the caller's wishlist.
fn add_to_wishlist(state: PhysPanelState, url: String, uuid: String) {
    let PhysPanelState {
        mut wishlist,
        mut busy,
        mut err,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        match data::add_wishlist_entry(&url, &uuid).await {
            Ok(entry) => wishlist.set(Some(entry)),
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

/// Remove the book from the caller's wishlist.
fn remove_from_wishlist(state: PhysPanelState, url: String, uuid: String) {
    let PhysPanelState {
        mut wishlist,
        mut busy,
        mut err,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        match data::remove_wishlist_entry(&url, &uuid).await {
            Ok(()) => wishlist.set(None),
            Err(e) => err.set(Some(e.to_string())),
        }
        busy.set(false);
    });
}

// Pure helpers.

/// Relative "N ago" phrase from unix seconds against an injected `now`, so the
/// formatter is clock-injectable and unit-testable.
fn time_ago(now: i64, ts: i64) -> String {
    let secs = (now - ts).max(0);
    let plural = |n: i64| if n == 1 { "" } else { "s" };
    if secs < 60 {
        "just now".to_string()
    } else if secs < 3600 {
        let m = secs / 60;
        format!("{m} minute{} ago", plural(m))
    } else if secs < 86_400 {
        let h = secs / 3600;
        format!("{h} hour{} ago", plural(h))
    } else {
        let d = secs / 86_400;
        format!("{d} day{} ago", plural(d))
    }
}

/// "Checked in {relative}" label for a copy's check-in timestamp.
fn checked_in_label(now: i64, ts: i64) -> String {
    format!("Checked in {}", time_ago(now, ts))
}

/// Human phrase for where a wishlist entry originated.
fn source_label(source: WishlistSource) -> &'static str {
    match source {
        WishlistSource::Scan => "a scan",
        WishlistSource::Detail => "this page",
        WishlistSource::Manual => "manual entry",
    }
}

/// "Find a copy" target URL. A default web search until the per-user source
/// setting lands (#1188): the ISBN when present, else title + author.
fn find_a_copy_url(isbn: Option<&str>, title: &str, author: &str) -> String {
    let query = match isbn {
        Some(i) if !i.trim().is_empty() => i.trim().to_string(),
        _ => format!("{title} {author}").trim().to_string(),
    };
    format!("https://www.google.com/search?q={}", encode_query(&query))
}

/// Percent-encode a query string (RFC 3986 unreserved kept literal, the rest
/// `%XX`). Dependency-free so it compiles identically on wasm and SSR.
fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Current unix seconds. Only ever called client-side (labels render after the
/// post-mount load), so SSR never invokes the JS clock. Mirrors
/// `rating::now_unix`.
fn now_unix() -> i64 {
    #[cfg(feature = "web")]
    {
        (js_sys::Date::now() / 1000.0) as i64
    }
    #[cfg(not(feature = "web"))]
    {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_ago_shows_just_now_for_sub_minute_gap() {
        assert_eq!(time_ago(1_000, 999), "just now");
    }

    #[test]
    fn time_ago_singularizes_one_minute_and_pluralizes_more() {
        assert_eq!(time_ago(1_060, 1_000), "1 minute ago");
        assert_eq!(time_ago(1_000 + 3 * 60, 1_000), "3 minutes ago");
    }

    #[test]
    fn time_ago_pluralizes_hours_and_days() {
        assert_eq!(time_ago(1_000 + 2 * 3600, 1_000), "2 hours ago");
        assert_eq!(time_ago(172_800, 86_400), "1 day ago");
    }

    #[test]
    fn time_ago_clamps_future_timestamps_to_just_now() {
        assert_eq!(time_ago(1_000, 5_000), "just now");
    }

    #[test]
    fn checked_in_label_prefixes_the_relative_phrase() {
        assert_eq!(checked_in_label(1_060, 1_000), "Checked in 1 minute ago");
    }

    #[test]
    fn source_label_covers_every_variant() {
        assert_eq!(source_label(WishlistSource::Scan), "a scan");
        assert_eq!(source_label(WishlistSource::Detail), "this page");
        assert_eq!(source_label(WishlistSource::Manual), "manual entry");
    }

    #[test]
    fn find_a_copy_url_prefers_isbn_when_present() {
        let url = find_a_copy_url(Some("9780451524935"), "1984", "George Orwell");
        assert_eq!(url, "https://www.google.com/search?q=9780451524935");
    }

    #[test]
    fn find_a_copy_url_falls_back_to_title_and_author() {
        let url = find_a_copy_url(None, "Brave New World", "Aldous Huxley");
        assert_eq!(
            url,
            "https://www.google.com/search?q=Brave%20New%20World%20Aldous%20Huxley"
        );
    }

    #[test]
    fn find_a_copy_url_treats_blank_isbn_as_absent() {
        let url = find_a_copy_url(Some("   "), "Dune", "Frank Herbert");
        assert!(url.ends_with("Dune%20Frank%20Herbert"));
    }

    #[test]
    fn encode_query_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode_query("a-b_c.d~e"), "a-b_c.d~e");
        assert_eq!(encode_query("a b&c"), "a%20b%26c");
    }
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    /// The panel's first paint, before the post-mount load resolves.
    fn panel_first_paint() -> Element {
        rsx! {
            BdPhysicalPanel {
                uuid: "book-uuid".to_string(),
                is_fileless: false,
                isbn: None,
                title: "Dracula".to_string(),
                author: "Bram Stoker".to_string(),
                refresh: use_signal(|| 0u32),
            }
        }
    }

    #[test]
    fn panel_first_paint_is_empty_before_the_post_mount_load() {
        // The load effect hasn't resolved after one rebuild, so `loaded` stays
        // false and the panel emits nothing — exactly what the client hydrates
        // against (rule 07).
        let html = render_in_vdom(panel_first_paint);
        assert!(!html.contains("data-testid=\"bd-physical-panel\""));
    }

    /// Seed a state with one copy and render the physical section directly, so
    /// the loaded markup (pill + copy card) is covered without the async load.
    fn physical_section_preview() -> Element {
        let state = PhysPanelState {
            copies: use_signal(|| {
                vec![PhysicalCopy {
                    id: 1,
                    book_uuid: "u".to_string(),
                    isbn: Some("978".to_string()),
                    added_by_user_id: None,
                    checked_in_at: 0,
                    note: Some("First edition".to_string()),
                }]
            }),
            wishlist: use_signal(|| None),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target: use_signal(|| None),
            refresh: use_signal(|| 0u32),
        };
        render_physical_section(state, "".to_string(), "u".to_string(), false, true)
    }

    #[test]
    fn physical_section_renders_pill_copy_card_and_note() {
        let html = render_in_vdom(physical_section_preview);
        assert!(html.contains("data-testid=\"physical-pill\""));
        assert!(html.contains("In your physical collection"));
        assert!(html.contains("data-testid=\"physical-copy-card\""));
        assert!(html.contains("First edition"));
        // Edit-permitted, so both actions render.
        assert!(html.contains("data-testid=\"copy-edit-note\""));
        assert!(html.contains("data-testid=\"copy-delete\""));
    }

    /// Seed a wishlisted state and render its section (pill + tracking card).
    fn wishlist_section_preview() -> Element {
        let state = PhysPanelState {
            copies: use_signal(Vec::new),
            wishlist: use_signal(|| {
                Some(WishlistEntry {
                    id: 1,
                    user_id: 1,
                    book_uuid: "u".to_string(),
                    added_at: 0,
                    source: WishlistSource::Detail,
                })
            }),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target: use_signal(|| None),
            refresh: use_signal(|| 0u32),
        };
        render_wishlist_section(
            state,
            "".to_string(),
            "u".to_string(),
            None,
            "Dune".to_string(),
            "Frank Herbert".to_string(),
        )
    }

    #[test]
    fn wishlist_section_renders_pill_tracking_card_and_find_a_copy() {
        let html = render_in_vdom(wishlist_section_preview);
        assert!(html.contains("data-testid=\"wishlist-pill\""));
        assert!(html.contains("On your wishlist"));
        assert!(html.contains("data-testid=\"wishlist-card\""));
        assert!(html.contains("data-testid=\"wishlist-remove\""));
        assert!(html.contains("data-testid=\"find-a-copy\""));
    }

    fn state_with_target(last_fileless: bool) -> PhysPanelState {
        let mut delete_target = use_signal(|| None);
        delete_target.set(Some(DeleteTarget {
            copy_id: 7,
            last_fileless,
        }));
        PhysPanelState {
            copies: use_signal(Vec::new),
            wishlist: use_signal(|| None),
            busy: use_signal(|| false),
            err: use_signal(|| None),
            editing: use_signal(|| None),
            note_draft: use_signal(String::new),
            delete_target,
            refresh: use_signal(|| 0u32),
        }
    }

    fn simple_delete_modal_preview() -> Element {
        render_delete_modal(state_with_target(false), "".to_string(), "u".to_string())
    }

    fn last_copy_modal_preview() -> Element {
        render_delete_modal(state_with_target(true), "".to_string(), "u".to_string())
    }

    #[test]
    fn simple_delete_modal_offers_the_i_sold_it_confirm() {
        let html = render_in_vdom(simple_delete_modal_preview);
        assert!(html.contains("data-testid=\"copy-delete-modal\""));
        assert!(html.contains("data-testid=\"copy-delete-confirm\""));
        assert!(html.contains("I sold it"));
    }

    #[test]
    fn last_copy_modal_offers_remove_or_wishlist() {
        let html = render_in_vdom(last_copy_modal_preview);
        assert!(html.contains("data-testid=\"last-copy-modal\""));
        assert!(html.contains("data-testid=\"last-copy-remove\""));
        assert!(html.contains("data-testid=\"last-copy-wishlist\""));
    }
}
