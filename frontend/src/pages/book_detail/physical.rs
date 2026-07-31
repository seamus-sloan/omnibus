//! Web-only physical-collection + wishlist UI for the book-detail page: the
//! full-width checked-in-copies panel (edit-note / delete) and the rail-card
//! wishlist slot (tracking card / add-to-wishlist affordance) the hero embeds.
//! Self-loads post-mount for SSR/WASM hydration parity; bumps the page
//! `refresh` signal after mutations that change physical/visibility state.

use dioxus::prelude::*;
use omnibus_shared::physical::{PhysicalCopy, WishlistEntry, WishlistSource};

use crate::components::{confirm_modal_body, ConfirmModal, ConfirmModalAction, ConfirmModalTone};
use crate::time::now_unix;
use crate::{data, use_server_url};

use super::{BdSectionHead, PhysSignals};

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

/// Book-identity fields the rail wishlist slot needs, bundled to keep
/// [`BdWishlistRailSlot`] under the 5-prop guideline (mirrors
/// `body::BdPageCtx`). `isbn`/`title`/`author` feed the "Find a copy" search;
/// `has_physical` suppresses the add affordance for an already-owned book.
#[derive(Clone, PartialEq, Props)]
pub(super) struct BdBookIdentity {
    pub uuid: String,
    pub has_physical: bool,
    pub isbn: Option<String>,
    pub title: String,
    pub author: String,
}

/// Checked-in physical-copies panel. Owns the post-mount copies+wishlist load
/// (writing the shared `phys` signals the hero's chip and rail slot read);
/// the wishlist UI itself lives in [`BdWishlistRailSlot`]. `refresh` is the
/// page's book-refetch signal.
#[component]
pub(super) fn BdPhysicalPanel(
    uuid: String,
    is_fileless: bool,
    refresh: Signal<u32>,
    phys: PhysSignals,
) -> Element {
    let server_url = use_server_url();
    let user = crate::use_current_user_summary();
    let can_edit = user().map(|u| u.is_admin || u.can_edit).unwrap_or(false);

    let copies = use_signal(Vec::<PhysicalCopy>::new);
    let wishlist = phys.wishlist;
    let loaded = phys.loaded;
    let err = use_signal(|| None::<String>);
    let state = PhysPanelState {
        copies,
        wishlist,
        busy: use_signal(|| false),
        err,
        editing: use_signal(|| None::<i64>),
        note_draft: use_signal(String::new),
        delete_target: use_signal(|| None::<DeleteTarget>),
        refresh,
    };

    use_physical_load_effect(
        uuid.clone(),
        server_url.clone(),
        copies,
        wishlist,
        err,
        loaded,
    );

    // First paint (SSR + first WASM) renders nothing; the post-mount load fills
    // it in. No hooks past this point, so the early returns keep hook order
    // fixed across the empty and loaded renders (rule 07). A copy-less book
    // without a load error renders nothing here either — its wishlist state
    // shows in the hero rail instead.
    if !loaded() || (copies().is_empty() && state.err.read().is_none()) {
        return rsx! {};
    }

    rsx! {
        section { class: "bd-physical-panel", "data-testid": "bd-physical-panel",
            if !copies().is_empty() {
                {render_physical_section(state, server_url.clone(), is_fileless, can_edit)}
            }
            if let Some(e) = state.err.read().clone() {
                p { role: "alert", class: "bd-phys-error", "data-testid": "physical-error", "{e}" }
            }
            {render_delete_modal(state, server_url, uuid)}
        }
    }
}

/// Rail-card wishlist slot the hero embeds under the rating + reading-status
/// blocks: the tracking card for a wishlisted book, or the add-to-wishlist
/// affordance for a book with no physical copy. Renders nothing until the
/// panel's shared post-mount load resolves (rule 07 — SSR and first WASM
/// paint both empty), and nothing for a book already in the collection.
#[component]
pub(super) fn BdWishlistRailSlot(identity: BdBookIdentity, phys: PhysSignals) -> Element {
    let BdBookIdentity {
        uuid,
        has_physical,
        isbn,
        title,
        author,
    } = identity;
    let server_url = use_server_url();
    let wishlist = phys.wishlist;
    let busy = use_signal(|| false);
    let err = use_signal(|| None::<String>);

    // No hooks past this point (rule 07).
    if !(phys.loaded)() {
        return rsx! {};
    }
    let entry = wishlist.read().clone();
    if entry.is_none() && has_physical {
        return rsx! {};
    }

    let content = match entry {
        Some(e) => {
            let added = time_ago(now_unix(), e.added_at);
            let source = source_label(e.source);
            let find_url = find_a_copy_url(isbn.as_deref(), &title, &author);
            let remove_url = server_url.clone();
            let remove_uuid = uuid.clone();
            rsx! {
                div { class: "bd-wishlist-rail", "data-testid": "wishlist-card",
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
                            onclick: move |_| {
                                remove_from_wishlist(
                                    wishlist, busy, err, remove_url.clone(), remove_uuid.clone(),
                                )
                            },
                            "Remove from wishlist"
                        }
                    }
                }
            }
        }
        None => rsx! {
            div { class: "bd-wishlist-rail", "data-testid": "wishlist-add-card",
                p { class: "bd-wishlist-add-blurb", "Track this title to find a physical copy later." }
                button {
                    class: "btn sm",
                    "data-testid": "add-to-wishlist",
                    disabled: busy(),
                    onclick: move |_| add_to_wishlist(wishlist, busy, err, server_url.clone(), uuid.clone()),
                    "Add to physical wishlist"
                }
            }
        },
    };

    rsx! {
        div { class: "divider" }
        div { class: "label bd-wishlist-head", "Physical wishlist" }
        {content}
        // Distinct testid from the panel's `physical-error`: both surfaces can
        // error at once (load failure + failed add/remove), and a shared id
        // would make Playwright selectors ambiguous.
        if let Some(e) = err.read().clone() {
            p { role: "alert", class: "bd-phys-error", "data-testid": "wishlist-error", "{e}" }
        }
    }
}

/// Install the uuid-reactive load effect: fetch this book's physical copies
/// and wishlist entry post-mount, resetting state first so a previous book's
/// copies/wishlist don't flash under the new book before its load resolves.
fn use_physical_load_effect(
    uuid: String,
    load_url: String,
    mut copies: Signal<Vec<PhysicalCopy>>,
    mut wishlist: Signal<Option<WishlistEntry>>,
    mut err: Signal<Option<String>>,
    mut loaded: Signal<bool>,
) {
    use_effect(use_reactive!(|uuid| {
        copies.set(Vec::new());
        wishlist.set(None);
        err.set(None);
        loaded.set(false);
        if uuid.is_empty() {
            return;
        }
        let load_url = load_url.clone();
        let uuid = uuid.clone();
        spawn(async move {
            // Surface a read failure rather than silently degrading to the
            // empty "add to wishlist" state (which would mask a 500/transient).
            match data::list_physical_copies(&load_url, &uuid).await {
                Ok(c) => copies.set(c),
                Err(e) => err.set(Some(e.to_string())),
            }
            match data::get_wishlist_entry(&load_url, &uuid).await {
                Ok(w) => wishlist.set(w),
                Err(e) => err.set(Some(e.to_string())),
            }
            loaded.set(true);
        });
    }));
}

/// The checked-in-copies section: pill + one card per copy.
fn render_physical_section(
    state: PhysPanelState,
    url: String,
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
                {render_copy_card(state, url.clone(), copy, is_fileless, can_edit)}
            }
        }
    }
}

/// One physical-copy card: check-in date, ISBN, note (view or inline editor),
/// and the edit/delete actions (edit-gated).
fn render_copy_card(
    state: PhysPanelState,
    url: String,
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
        div {
            key: "{copy.id}",
            class: "card bd-phys-copy",
            "data-testid": "physical-copy-card",
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
    let is_busy = busy();
    if target.last_fileless {
        let (uw, ww) = (url.clone(), uuid.clone());
        let title = "Remove your last copy";
        rsx! {
            ConfirmModal {
                testid: "last-copy-modal".to_string(),
                aria_label: title.to_string(),
                dialog_class: "mg-modal del-modal".to_string(),
                busy: is_busy,
                on_dismiss: move |_| delete_target.set(None),
                {confirm_modal_body(
                    title,
                    "This is the only copy of a book with no files in your library. Remove it entirely, or keep tracking it on your wishlist?",
                    vec![
                        ConfirmModalAction {
                            testid: "last-copy-cancel".to_string(),
                            label: "Cancel".to_string(),
                            tone: ConfirmModalTone::Ghost,
                            disabled: is_busy,
                            on_click: EventHandler::new(move |_| delete_target.set(None)),
                        },
                        ConfirmModalAction {
                            testid: "last-copy-wishlist".to_string(),
                            label: "Move to wishlist".to_string(),
                            tone: ConfirmModalTone::Ghost,
                            disabled: is_busy,
                            on_click: EventHandler::new(move |_| {
                                delete_last_and_wishlist(state, uw.clone(), ww.clone(), copy_id)
                            }),
                        },
                        ConfirmModalAction {
                            testid: "last-copy-remove".to_string(),
                            label: "Remove from library".to_string(),
                            tone: ConfirmModalTone::Danger,
                            disabled: is_busy,
                            on_click: EventHandler::new(move |_| {
                                delete_last_and_remove(state, url.clone(), uuid.clone(), copy_id)
                            }),
                        },
                    ],
                )}
            }
        }
    } else {
        let title = "Remove this copy?";
        rsx! {
            ConfirmModal {
                testid: "copy-delete-modal".to_string(),
                aria_label: title.to_string(),
                dialog_class: "mg-modal del-modal".to_string(),
                busy: is_busy,
                on_dismiss: move |_| delete_target.set(None),
                {confirm_modal_body(
                    title,
                    "This removes the physical copy from your collection.",
                    vec![
                        ConfirmModalAction {
                            testid: "copy-delete-cancel".to_string(),
                            label: "Cancel".to_string(),
                            tone: ConfirmModalTone::Ghost,
                            disabled: is_busy,
                            on_click: EventHandler::new(move |_| delete_target.set(None)),
                        },
                        ConfirmModalAction {
                            testid: "copy-delete-confirm".to_string(),
                            label: "I sold it".to_string(),
                            tone: ConfirmModalTone::Danger,
                            disabled: is_busy,
                            on_click: EventHandler::new(move |_| {
                                delete_copy_only(state, url.clone(), copy_id)
                            }),
                        },
                    ],
                )}
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

/// Shared shape of the two last-copy actions: delete the copy, drop it from
/// local state, close the modal, then run `follow_up` for the effect specific
/// to "remove from library" vs "move to wishlist". The copy leaves local state
/// the instant its own delete succeeds — before `follow_up` runs — so a failed
/// follow-up can't resurrect a phantom card or leave a stale one; it can only
/// drop the panel into a state the user can retry from.
fn delete_last_copy_then<F, Fut>(state: PhysPanelState, url: String, copy_id: i64, follow_up: F)
where
    F: FnOnce() -> Fut + 'static,
    Fut: std::future::Future<Output = ()> + 'static,
{
    let PhysPanelState {
        mut copies,
        mut busy,
        mut err,
        mut delete_target,
        ..
    } = state;
    busy.set(true);
    err.set(None);
    spawn(async move {
        if let Err(e) = data::delete_physical_copy(&url, copy_id).await {
            err.set(Some(e.to_string()));
            busy.set(false);
            return;
        }
        copies.with_mut(|l| l.retain(|c| c.id != copy_id));
        delete_target.set(None);
        follow_up().await;
        busy.set(false);
    });
}

/// Last-copy "Remove from library": delete the copy, then the now-fileless
/// book. Bumps `refresh` only on full success (book gone → the page re-renders
/// not-found).
fn delete_last_and_remove(state: PhysPanelState, url: String, uuid: String, copy_id: i64) {
    let PhysPanelState {
        mut refresh,
        mut err,
        ..
    } = state;
    let book_url = url.clone();
    delete_last_copy_then(state, url, copy_id, move || async move {
        match data::delete_fileless_book(&book_url, &uuid).await {
            Ok(()) => refresh.with_mut(|r| *r += 1),
            Err(e) => err.set(Some(e.to_string())),
        }
    });
}

/// Last-copy "Move to wishlist": delete the copy, then wishlist the book.
fn delete_last_and_wishlist(state: PhysPanelState, url: String, uuid: String, copy_id: i64) {
    let PhysPanelState {
        mut wishlist,
        mut err,
        ..
    } = state;
    let entry_url = url.clone();
    delete_last_copy_then(state, url, copy_id, move || async move {
        match data::add_wishlist_entry(&entry_url, &uuid).await {
            Ok(entry) => wishlist.set(Some(entry)),
            Err(e) => err.set(Some(e.to_string())),
        }
    });
}

/// Add the book to the caller's wishlist. Takes the individual signals (not
/// `PhysPanelState`) because its caller is the rail slot, not the panel.
fn add_to_wishlist(
    mut wishlist: Signal<Option<WishlistEntry>>,
    mut busy: Signal<bool>,
    mut err: Signal<Option<String>>,
    url: String,
    uuid: String,
) {
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

/// Remove the book from the caller's wishlist. Same signal-taking shape as
/// [`add_to_wishlist`].
fn remove_from_wishlist(
    mut wishlist: Signal<Option<WishlistEntry>>,
    mut busy: Signal<bool>,
    mut err: Signal<Option<String>>,
    url: String,
    uuid: String,
) {
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

/// "Find a copy" target URL: an Amazon search on the ISBN when present, else
/// on title + author.
fn find_a_copy_url(isbn: Option<&str>, title: &str, author: &str) -> String {
    let query = match isbn {
        Some(i) if !i.trim().is_empty() => i.trim().to_string(),
        _ => format!("{title} {author}").trim().to_string(),
    };
    format!("https://www.amazon.com/s?k={}", encode_query(&query))
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

#[cfg(test)]
mod tests;
