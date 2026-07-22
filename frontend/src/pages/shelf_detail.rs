//! Shelf detail page (`/shelves/:id`).
//!
//! Renders the shelf rail alongside a main column showing one shelf's header
//! (kind + visibility badges, actions) and its member books. Smart shelves
//! show their rule as chips and an auto-sorted grid; manual shelves show a
//! position-ordered grid with an "Add books" affordance.

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use dioxus_router::use_navigator;
use dioxus_router::Link;
// `ShelfKind` / `Visibility` are consumed only by the web rail/header path;
// the mobile branch renders `mobile::MobileShelfDetail`, which imports its own.
use omnibus_shared::{EbookMetadata, RuleField, RuleOp, Shelf, ShelfRule, SortDir, SortKey};
#[cfg(not(feature = "mobile"))]
use omnibus_shared::{ShelfKind, UpdateShelfRequest, Visibility};

#[cfg(not(feature = "mobile"))]
use crate::components::atrium::fallback_title;
use crate::components::cover_tile::toggle_picked;
use crate::components::EditShelfRulesModal;
use crate::components::{CoverTile, CoverTileKind};
#[cfg(not(feature = "mobile"))]
use crate::components::{RailActive, ShelvesRail};
use crate::{data, use_server_url, Route};

#[cfg(feature = "mobile")]
mod mobile;

/// Shelf detail page — see the module doc for the smart/manual split.
#[component]
pub fn ShelfDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let shelf = use_signal(|| None::<Shelf>);
    let books = use_signal(Vec::<EbookMetadata>::new);
    let loading = use_signal(|| true);
    let error = use_signal(|| None::<String>);
    crate::use_page_title(move || shelf.read().as_ref().map(|s| s.name.clone()));
    let sort_key = use_signal(|| SortKey::Title);
    // Mobile mutates these directly (see the mobile `body` branch below);
    // web only threads them by value into `web_shelf_body`/`shelf_detail_modals`,
    // so only the mobile build needs the bindings themselves to be `mut`.
    #[cfg(feature = "mobile")]
    let mut show_add = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let show_add = use_signal(|| false);
    #[cfg(feature = "mobile")]
    let mut edit_rules = use_signal(|| false);
    #[cfg(not(feature = "mobile"))]
    let edit_rules = use_signal(|| false);
    // Bumped to force a refetch after a membership edit.
    #[cfg(feature = "mobile")]
    let mut reload = use_signal(|| 0u32);
    #[cfg(not(feature = "mobile"))]
    let reload = use_signal(|| 0u32);
    // Set when the member-books refetch fails, so a transient network error
    // renders distinctly from a shelf that is genuinely empty (mirrors
    // `search_mobile.rs`'s `errored` signal for the same failure class).
    let errored = use_signal(|| false);

    use_shelf_effects(
        id,
        server_url.clone(),
        sort_key,
        reload,
        ShelfFetchSignals {
            shelf,
            loading,
            error,
            books,
            errored,
        },
    );

    if loading() && shelf.read().is_none() {
        return render_page_state(id, rsx! { p { class: "subtitle", "Loading\u{2026}" } });
    }

    let Some(current) = shelf.read().clone() else {
        return render_page_state(
            id,
            rsx! {
                p { role: "alert", class: "subtitle",
                    {error().unwrap_or_else(|| "Shelf not found.".into())}
                }
                Link { to: Route::Landing {}, class: "btn", "Back to All books" }
            },
        );
    };

    // Web keeps the rail + toolbar layout; mobile renders the home-style
    // full-screen surface. Both consume the shared fetch pipeline above.
    // (Mobile is a separate build — rule 07 hydration parity is unaffected.)
    #[cfg(feature = "mobile")]
    let body = rsx! {
        mobile::MobileShelfDetail {
            shelf: current.clone(),
            books: books.read().clone(),
            errored: errored(),
            server_url: server_url.clone(),
            on_add: move |_| show_add.set(true),
            on_edit_rules: move |_| edit_rules.set(true),
            on_changed: move |_| reload.with_mut(|n| *n += 1),
        }
    };

    #[cfg(not(feature = "mobile"))]
    let body = web_shelf_body(
        &current,
        &books.read(),
        errored(),
        &server_url,
        ShelfBodySignals {
            sort_key,
            show_add,
            edit_rules,
            reload,
        },
    );

    rsx! {
        {body}
        {shelf_detail_modals(id, current, show_add, edit_rules, reload)}
    }
}

/// The "Add books" and "Edit rules" modals, shown when their respective
/// signals flip true; both bump `reload` on success so the parent refetches.
fn shelf_detail_modals(
    shelf_id: i64,
    current: Shelf,
    mut show_add: Signal<bool>,
    mut edit_rules: Signal<bool>,
    mut reload: Signal<u32>,
) -> Element {
    rsx! {
        if show_add() {
            AddBooksModal {
                shelf_id,
                on_close: move |_| show_add.set(false),
                on_added: move |_| {
                    show_add.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }

        if edit_rules() {
            EditShelfRulesModal {
                shelf: current.clone(),
                on_close: move |_| edit_rules.set(false),
                on_saved: move |_| {
                    edit_rules.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }
    }
}

/// Data signals populated by the shelf detail fetch effects. `Copy` (Dioxus
/// signals), so grouping them keeps [`use_shelf_effects`] under the
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
struct ShelfFetchSignals {
    shelf: Signal<Option<Shelf>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    books: Signal<Vec<EbookMetadata>>,
    errored: Signal<bool>,
}

/// Wires the two data-fetch effects backing [`ShelfDetailPage`]: the shelf
/// detail itself (id-driven) and its member books (id/sort/reload-driven).
/// Extracted to keep the page component under the line cap; mirrors
/// `book_detail::use_book_data_effects`.
fn use_shelf_effects(
    id: i64,
    server_url: String,
    sort_key: Signal<SortKey>,
    reload: Signal<u32>,
    sig: ShelfFetchSignals,
) {
    let ShelfFetchSignals {
        mut shelf,
        mut loading,
        mut error,
        mut books,
        mut errored,
    } = sig;

    // Fetch the shelf detail whenever the id changes. `id` is a plain prop
    // (not a signal), so it must be wrapped in `use_reactive!` to re-arm this
    // effect on navigation between shelves — see `BookDetailPage` for why.
    let shelf_url = server_url.clone();
    use_effect(use_reactive!(|id| {
        let url = shelf_url.clone();
        let _ = reload();
        spawn(async move {
            loading.set(true);
            match data::get_shelf(&url, id).await {
                Ok(s) => {
                    shelf.set(Some(s));
                    error.set(None);
                }
                Err(e) => {
                    shelf.set(None);
                    error.set(Some(e.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    // Fetch the member books, re-running on id/sort change or a membership
    // edit. `sort_key()` is a signal read, already tracked; `id` needs the
    // same `use_reactive!` wrapping as above.
    let page_url = server_url;
    use_effect(use_reactive!(|id| {
        let url = page_url.clone();
        let key = sort_key();
        let _ = reload();
        spawn(async move {
            let dir = default_dir_for(key);
            match data::shelf_page(&url, id, key, dir).await {
                Ok(page) => {
                    books.set(page.books);
                    errored.set(false);
                }
                Err(_) => {
                    // `tracing` isn't linked under the `web` (WASM) feature,
                    // so the signal alone carries the failure to the UI.
                    // Clear the stale list too — otherwise a refetch failure
                    // after navigating shelves leaves the prior shelf's
                    // books on screen under the error banner.
                    books.set(Vec::new());
                    errored.set(true);
                }
            }
        });
    }));
}

/// Loading / not-found chrome. Web wraps in the rail layout; mobile renders
/// the bare screen surface.
fn render_page_state(id: i64, inner: Element) -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        rsx! {
            div { class: "shelf-layout",
                ShelvesRail { active: RailActive::Shelf(id) }
                div { class: "shelf-main", {inner} }
            }
        }
    }
    #[cfg(feature = "mobile")]
    {
        let _ = id;
        rsx! {
            div { class: "m-shelves", {inner} }
        }
    }
}

/// UI-state signals threaded into [`web_shelf_body`]. `Copy` (Dioxus
/// signals), so grouping them keeps the function under clippy's
/// too-many-arguments cap without changing call-site ergonomics.
#[cfg(not(feature = "mobile"))]
#[derive(Clone, Copy)]
struct ShelfBodySignals {
    sort_key: Signal<SortKey>,
    show_add: Signal<bool>,
    edit_rules: Signal<bool>,
    reload: Signal<u32>,
}

/// Web presentation: rail + header (badges/actions), rule chips + sort row
/// for smart shelves, and the `CoverTile` member grid.
#[cfg(not(feature = "mobile"))]
fn web_shelf_body(
    current: &Shelf,
    books: &[EbookMetadata],
    errored: bool,
    server_url: &str,
    signals: ShelfBodySignals,
) -> Element {
    let ShelfBodySignals {
        mut sort_key,
        mut show_add,
        mut edit_rules,
        mut reload,
    } = signals;
    let id = current.id;
    let is_smart = current.kind == ShelfKind::Smart;
    rsx! {
        div { class: "shelf-layout",
            ShelvesRail { active: RailActive::Shelf(id) }
            div { class: "shelf-main",
                ShelfHeader {
                    shelf: current.clone(),
                    on_edit_rules: move |_| edit_rules.set(true),
                    on_changed: move |_| reload.with_mut(|n| *n += 1),
                }

                if is_smart {
                    div { class: "shelf-rule-summary",
                        for (i, rule) in current.rules.iter().enumerate() {
                            span { key: "{i}", class: "shelf-rule-chip", "{rule_text(rule)}" }
                        }
                    }
                    div { class: "shelf-sort-row",
                        span { class: "label", "Sort" }
                        select {
                            class: "shelf-select",
                            "data-testid": "shelf-sort",
                            value: sort_key().as_wire(),
                            onchange: move |e| {
                                if let Some(k) = SortKey::from_wire(&e.value()) {
                                    sort_key.set(k);
                                }
                            },
                            option { value: "title", "Title" }
                            option { value: "author", "Author" }
                            option { value: "series", "Series" }
                            option { value: "newest_added", "Newest added" }
                            option { value: "last_updated", "Recently updated" }
                        }
                    }
                } else if let Some(desc) = current.description.as_ref() {
                    p { class: "shelf-desc", "{desc}" }
                }

                if errored {
                    p {
                        role: "alert",
                        class: "error",
                        "data-testid": "shelf-refetch-error",
                        "Couldn\u{2019}t refresh this shelf. Check your connection and try again."
                    }
                }

                {member_grid(books, server_url, is_smart, move |_| show_add.set(true))}
            }
        }
    }
}

/// Member-books grid; manual shelves get a trailing dashed "Add books" tile.
#[cfg(not(feature = "mobile"))]
fn member_grid(
    books: &[EbookMetadata],
    server_url: &str,
    is_smart: bool,
    on_add: impl FnMut(()) + Clone + 'static,
) -> Element {
    rsx! {
        div { class: "lib-grid shelf-grid", "data-testid": "shelf-grid", role: "list",
            for book in books.iter().cloned() {
                {
                    // Match `Cover`'s empty-title handling (issue #92): blank /
                    // whitespace-only titles fall back to the filename so the
                    // caption + aria-label never render empty.
                    let title = fallback_title(book.title.as_deref(), &book.filename);
                    rsx! {
                        div {
                            key: "{book.id}",
                            CoverTile {
                                book,
                                server_url: server_url.to_string(),
                                sizes: "(max-width: 640px) 160px, 200px".to_string(),
                                kind: CoverTileKind::MemberLink { title },
                            }
                        }
                    }
                }
            }
            if !is_smart {
                {
                    let mut on_add = on_add.clone();
                    rsx! {
                        button {
                            r#type: "button",
                            class: "shelf-add-tile",
                            "data-testid": "shelf-add-books",
                            onclick: move |_| on_add(()),
                            span { class: "shelf-add-tile-plus", "\u{FF0B}" }
                            span { "Add books" }
                        }
                    }
                }
            }
        }
    }
}

/// Header: back link, badges, name, and the actions menu. `on_changed` fires
/// after a successful rename / visibility change so the parent refetches and
/// the header reflects the new value; `on_edit_rules` opens the smart-shelf
/// rule editor.
#[cfg(not(feature = "mobile"))]
#[component]
fn ShelfHeader(
    shelf: Shelf,
    on_edit_rules: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let renaming = use_signal(|| false);
    let mut draft_name = use_signal(|| shelf.name.clone());
    let menu_open = use_signal(|| false);

    // Owner/admin gating; `None` until the boot effect resolves the viewer, so
    // controls stay hidden on SSR + first paint (hydration parity, rule 07). The
    // server enforces the same rule (`shelf_for_edit`).
    let viewer = crate::use_current_user_summary()();
    let owner_id = shelf.owner_user_id;
    let can_manage = viewer
        .as_ref()
        .is_some_and(|u| u.id == owner_id || u.is_admin);
    let show_attribution = viewer.as_ref().is_some_and(|u| u.id != owner_id);

    let id = shelf.id;
    let is_smart = shelf.kind == ShelfKind::Smart;
    let kind_label = match shelf.kind {
        ShelfKind::Smart => "Smart",
        ShelfKind::Manual => "Hand-picked",
    };
    let vis_label = match shelf.visibility {
        Visibility::Private => "Private",
        Visibility::Public => "Public",
    };
    let next_vis = match shelf.visibility {
        Visibility::Private => Visibility::Public,
        Visibility::Public => Visibility::Private,
    };

    let on_delete = build_on_delete(server_url.clone(), id, nav);
    let on_toggle_vis =
        build_on_toggle_vis(server_url.clone(), id, next_vis, menu_open, on_changed);
    let on_rename_save =
        build_on_rename_save(server_url.clone(), id, renaming, draft_name, on_changed);

    rsx! {
        header { class: "shelf-detail-header", "data-testid": "shelf-detail-header",
            Link { to: Route::Landing {}, class: "shelf-back", "\u{2190} All books" }
            div { class: "shelf-badges",
                span { class: "shelf-badge", "{kind_label}" }
                span { class: "shelf-badge shelf-badge--vis", "{vis_label}" }
                if show_attribution {
                    span {
                        class: "shelf-badge shelf-badge--owner",
                        "data-testid": "shelf-owner-attribution",
                        "by {shelf.owner_username}"
                    }
                }
            }
            div { class: "shelf-title-row",
                if renaming() {
                    input {
                        r#type: "text",
                        class: "shelf-name-input",
                        "data-testid": "shelf-rename-input",
                        value: "{draft_name}",
                        oninput: move |e| draft_name.set(e.value()),
                    }
                    button {
                        r#type: "button", class: "btn shelf-btn-primary",
                        "data-testid": "shelf-rename-save",
                        onclick: move |_| on_rename_save.call(()),
                        "Save"
                    }
                } else {
                    h1 { class: "shelf-title", "{shelf.name}" }
                    if can_manage {
                        {shelf_actions_menu(
                            is_smart,
                            menu_open,
                            renaming,
                            on_edit_rules,
                            on_toggle_vis,
                            on_delete,
                        )}
                    }
                }
            }
        }
    }
}

/// Builds the delete-shelf handler: deletes then navigates back to the
/// library on success.
#[cfg(not(feature = "mobile"))]
fn build_on_delete(server_url: String, id: i64, nav: dioxus_router::Navigator) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        spawn(async move {
            if data::delete_shelf(&url, id).await.is_ok() {
                nav.push(Route::Landing {});
            }
        });
    })
}

/// Builds the visibility-toggle handler: closes the actions menu, flips
/// visibility, and refetches on success.
#[cfg(not(feature = "mobile"))]
fn build_on_toggle_vis(
    server_url: String,
    id: i64,
    next_vis: Visibility,
    mut menu_open: Signal<bool>,
    on_changed: EventHandler<()>,
) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        menu_open.set(false);
        spawn(async move {
            let req = UpdateShelfRequest {
                visibility: Some(next_vis),
                ..Default::default()
            };
            if data::update_shelf(&url, id, req).await.is_ok() {
                on_changed.call(());
            }
        });
    })
}

/// Builds the rename-save handler: saves the draft name and refetches on
/// success.
#[cfg(not(feature = "mobile"))]
fn build_on_rename_save(
    server_url: String,
    id: i64,
    mut renaming: Signal<bool>,
    draft_name: Signal<String>,
    on_changed: EventHandler<()>,
) -> EventHandler<()> {
    EventHandler::new(move |()| {
        let url = server_url.clone();
        let name = draft_name();
        renaming.set(false);
        spawn(async move {
            let req = UpdateShelfRequest {
                name: Some(name),
                ..Default::default()
            };
            if data::update_shelf(&url, id, req).await.is_ok() {
                on_changed.call(());
            }
        });
    })
}

/// Owner-only actions trigger + dropdown: edit rules (smart shelves only),
/// rename, toggle visibility, delete. Split out of [`ShelfHeader`] to keep
/// it under the line cap, mirroring `member_grid`'s plain-fn split.
#[cfg(not(feature = "mobile"))]
fn shelf_actions_menu(
    is_smart: bool,
    mut menu_open: Signal<bool>,
    mut renaming: Signal<bool>,
    on_edit_rules: EventHandler<()>,
    on_toggle_vis: EventHandler<()>,
    on_delete: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "shelf-actions",
            button {
                r#type: "button",
                class: "shelf-actions-btn",
                "data-testid": "shelf-actions",
                "aria-label": "Shelf actions",
                onclick: move |_| menu_open.toggle(),
                "\u{22EF}"
            }
            if menu_open() {
                div { class: "shelf-actions-menu",
                    if is_smart {
                        button {
                            r#type: "button", class: "shelf-menu-item",
                            "data-testid": "shelf-edit-rules",
                            onclick: move |_| { menu_open.set(false); on_edit_rules.call(()); },
                            "Edit rules"
                        }
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item",
                        "data-testid": "shelf-rename",
                        onclick: move |_| { menu_open.set(false); renaming.set(true); },
                        "Rename"
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item",
                        "data-testid": "shelf-toggle-visibility",
                        onclick: move |_| on_toggle_vis.call(()),
                        "Change visibility"
                    }
                    button {
                        r#type: "button", class: "shelf-menu-item shelf-menu-item--danger",
                        "data-testid": "shelf-delete",
                        onclick: move |_| on_delete.call(()),
                        "Delete"
                    }
                }
            }
        }
    }
}

/// Modal that appends library books to an existing manual shelf.
#[component]
fn AddBooksModal(shelf_id: i64, on_close: EventHandler<()>, on_added: EventHandler<()>) -> Element {
    let server_url = use_server_url();
    let library = use_signal(Vec::<EbookMetadata>::new);
    let mut query = use_signal(String::new);
    let picked = use_signal(Vec::<String>::new);
    let mut saving = use_signal(|| false);

    use_library_fetch(server_url.clone(), library);

    let add_url = server_url.clone();
    let on_add = move |_| {
        if saving() || picked.read().is_empty() {
            return;
        }
        let url = add_url.clone();
        let uuids = picked.read().clone();
        let on_added = on_added;
        saving.set(true);
        spawn(async move {
            if data::add_shelf_books(&url, shelf_id, uuids).await.is_ok() {
                on_added.call(());
            }
            saving.set(false);
        });
    };

    let library_books = library.read();
    let filtered = filter_library(&library_books, &query.read());
    let picked_count = picked.read().len();

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "add-books-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),
                div { class: "shelf-modal-body",
                    div { class: "shelf-picker-bar",
                        input {
                            r#type: "search",
                            class: "shelf-picker-search",
                            placeholder: "Search your library\u{2026}",
                            "data-testid": "add-books-search",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                        span { class: "mono shelf-picker-count", "Selected \u{b7} {picked_count}" }
                    }
                    {add_books_picker_grid(&filtered, &server_url, picked)}
                }
                div { class: "shelf-modal-foot",
                    button {
                        r#type: "button", class: "btn shelf-btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button", class: "btn shelf-btn-primary",
                        "data-testid": "add-books-submit",
                        disabled: saving(),
                        onclick: on_add,
                        if saving() { "Adding\u{2026}" } else { "Add \u{b7} {picked_count}" }
                    }
                }
            }
        }
    }
}

/// Fetches the full library once on mount, for the "Add books" search/pick UI.
fn use_library_fetch(server_url: String, mut library: Signal<Vec<EbookMetadata>>) {
    use_effect(move || {
        let url = server_url.clone();
        spawn(async move {
            if let Ok(lib) = data::get_ebooks(&url).await {
                library.set(lib.books);
            }
        });
    });
}

/// Library books whose title (or filename fallback) contains `query`,
/// case-insensitively; an empty query matches everything.
fn filter_library<'a>(books: &'a [EbookMetadata], query: &str) -> Vec<&'a EbookMetadata> {
    let q = query.to_lowercase();
    books
        .iter()
        .filter(|b| {
            q.is_empty()
                || b.title
                    .as_deref()
                    .unwrap_or(&b.filename)
                    .to_lowercase()
                    .contains(&q)
        })
        .collect()
}

/// Selectable cover grid for the "Add books" modal; toggles membership in
/// `picked` via `CoverTile`'s `Selectable` kind.
fn add_books_picker_grid(
    filtered: &[&EbookMetadata],
    server_url: &str,
    picked: Signal<Vec<String>>,
) -> Element {
    rsx! {
        div { class: "shelf-picker-grid",
            for book in filtered.iter().map(|b| (*b).clone()) {
                {
                    let uuid = book.unique_identifier.clone().unwrap_or_default();
                    let selected = picked.read().contains(&uuid);
                    let mut picked = picked;
                    rsx! {
                        div {
                            key: "{book.id}",
                            CoverTile {
                                book,
                                server_url: server_url.to_string(),
                                sizes: "120px".to_string(),
                                kind: CoverTileKind::Selectable {
                                    selected,
                                    on_toggle: EventHandler::new(move |_| toggle_picked(&mut picked, &uuid)),
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The default sort direction the grid uses for a given sort axis: newest-first
/// for the date axes, ascending otherwise.
fn default_dir_for(key: SortKey) -> SortDir {
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

/// Human-readable summary of one smart rule, e.g. "Tag is Fantasy".
fn rule_text(rule: &ShelfRule) -> String {
    format!(
        "{} {} {}",
        field_label(rule.field),
        op_label(rule.op),
        rule.value
    )
}

/// Display label for a rule field.
fn field_label(field: RuleField) -> &'static str {
    match field {
        RuleField::Tag => "Tag",
        RuleField::Author => "Author",
        RuleField::Series => "Series",
        RuleField::Rating => "Rating",
        RuleField::Status => "Status",
        RuleField::Format => "Format",
        RuleField::Year => "Year",
        RuleField::DateAdded => "Date added",
        RuleField::DateUpdated => "Date updated",
    }
}

/// Display label for a rule operator.
fn op_label(op: RuleOp) -> &'static str {
    match op {
        RuleOp::Is => "is",
        RuleOp::IsNot => "is not",
        RuleOp::Contains => "contains",
        RuleOp::StartsWith => "starts with",
        RuleOp::Gte => "is at least",
        RuleOp::Includes => "includes",
        RuleOp::InLast => "in the last",
        RuleOp::Between => "between",
        RuleOp::Before => "before",
        RuleOp::After => "after",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_text_reads_field_op_value() {
        let rule = ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        };
        assert_eq!(rule_text(&rule), "Tag is Fantasy");
    }

    #[test]
    fn default_dir_for_dates_is_desc() {
        assert_eq!(default_dir_for(SortKey::NewestAdded), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::LastUpdated), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::Title), SortDir::Asc);
    }
}
