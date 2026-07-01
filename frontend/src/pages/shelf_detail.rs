//! Shelf detail page (`/shelves/:id`).
//!
//! Renders the shelf rail alongside a main column showing one shelf's header
//! (kind + visibility badges, actions) and its member books. Smart shelves
//! show their rule as chips and an auto-sorted grid; manual shelves show a
//! position-ordered grid with an "Add books" affordance.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use dioxus_router::Link;
use omnibus_shared::{
    EbookMetadata, RuleField, RuleOp, Shelf, ShelfKind, ShelfRule, SortDir, SortKey,
    UpdateShelfRequest, Visibility,
};

use crate::components::atrium::Cover;
use crate::components::{RailActive, ShelvesRail};
use crate::{data, use_server_url, Route};

/// Shelf detail page — see the module doc for the smart/manual split.
#[component]
pub fn ShelfDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let mut shelf = use_signal(|| None::<Shelf>);
    let mut books = use_signal(Vec::<EbookMetadata>::new);
    let mut loading = use_signal(|| true);
    let mut error = use_signal(|| None::<String>);
    let mut sort_key = use_signal(|| SortKey::Title);
    let mut show_add = use_signal(|| false);
    // Bumped to force a refetch after a membership edit.
    let mut reload = use_signal(|| 0u32);

    // Fetch the shelf detail whenever the id changes.
    let shelf_url = server_url.clone();
    use_effect(move || {
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
    });

    // Fetch the member books, re-running on sort change or a membership edit.
    let page_url = server_url.clone();
    use_effect(move || {
        let url = page_url.clone();
        let key = sort_key();
        let _ = reload();
        spawn(async move {
            let dir = default_dir_for(key);
            if let Ok(page) = data::shelf_page(&url, id, key, dir).await {
                books.set(page.books);
            }
        });
    });

    if loading() && shelf.read().is_none() {
        return rsx! {
            div { class: "shelf-layout",
                ShelvesRail { active: RailActive::Shelf(id) }
                div { class: "shelf-main",
                    p { class: "subtitle", "Loading\u{2026}" }
                }
            }
        };
    }

    let Some(current) = shelf.read().clone() else {
        return rsx! {
            div { class: "shelf-layout",
                ShelvesRail { active: RailActive::Shelf(id) }
                div { class: "shelf-main",
                    p { role: "alert", class: "subtitle",
                        {error().unwrap_or_else(|| "Shelf not found.".into())}
                    }
                    Link { to: Route::Landing {}, class: "btn", "Back to All books" }
                }
            }
        };
    };

    let is_smart = current.kind == ShelfKind::Smart;

    rsx! {
        div { class: "shelf-layout",
            ShelvesRail { active: RailActive::Shelf(id) }
            div { class: "shelf-main",
                ShelfHeader {
                    shelf: current.clone(),
                    on_changed: move |_| reload.with_mut(|n| *n += 1),
                }

                if is_smart {
                    div { class: "shelf-rule-summary",
                        for rule in current.rules.iter() {
                            span { class: "shelf-rule-chip", "{rule_text(rule)}" }
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

                {member_grid(&books.read(), &server_url, is_smart, move |_| show_add.set(true))}
            }
        }

        if show_add() {
            AddBooksModal {
                shelf_id: id,
                on_close: move |_| show_add.set(false),
                on_added: move |_| {
                    show_add.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }
    }
}

/// Member-books grid; manual shelves get a trailing dashed "Add books" tile.
fn member_grid(
    books: &[EbookMetadata],
    server_url: &str,
    is_smart: bool,
    on_add: impl FnMut(()) + Clone + 'static,
) -> Element {
    rsx! {
        div { class: "lib-grid shelf-grid", "data-testid": "shelf-grid", role: "list",
            for book in books.iter().cloned() {
                {member_tile(book, server_url)}
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

/// One member-book cover tile linking to the detail page. Uses [`Link`] (which
/// renders an `<a>` without a hook) rather than `use_navigator()` — this is a
/// plain fn rendered once per book, so calling a hook here would vary the
/// parent's hook count with the list length and break Dioxus's hook order.
fn member_tile(book: EbookMetadata, server_url: &str) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);

    rsx! {
        Link {
            to: Route::BookDetail { uuid: uuid.clone() },
            class: "cover-link lib-tile",
            role: "listitem",
            "data-testid": "shelf-tile",
            aria_label: "Open details for {title}",
            Cover {
                book,
                src_override: src,
                srcset,
                sizes: Some("(max-width: 640px) 160px, 200px".to_string()),
            }
            div { class: "lib-tile-title", "{title}" }
        }
    }
}

/// Header: back link, badges, name, and the actions menu. `on_changed` fires
/// after a successful rename / visibility change so the parent refetches and
/// the header reflects the new value.
#[component]
fn ShelfHeader(shelf: Shelf, on_changed: EventHandler<()>) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let mut renaming = use_signal(|| false);
    let mut draft_name = use_signal(|| shelf.name.clone());
    let mut menu_open = use_signal(|| false);

    let id = shelf.id;
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

    let del_url = server_url.clone();
    let on_delete = move |_| {
        let url = del_url.clone();
        spawn(async move {
            if data::delete_shelf(&url, id).await.is_ok() {
                nav.push(Route::Landing {});
            }
        });
    };

    let vis_url = server_url.clone();
    let on_toggle_vis = move |_| {
        let url = vis_url.clone();
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
    };

    let rename_url = server_url.clone();
    let on_rename_save = move |_| {
        let url = rename_url.clone();
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
    };

    rsx! {
        header { class: "shelf-detail-header", "data-testid": "shelf-detail-header",
            Link { to: Route::Landing {}, class: "shelf-back", "\u{2190} All books" }
            div { class: "shelf-badges",
                span { class: "shelf-badge", "{kind_label}" }
                span { class: "shelf-badge shelf-badge--vis", "{vis_label}" }
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
                        onclick: on_rename_save,
                        "Save"
                    }
                } else {
                    h1 { class: "shelf-title", "{shelf.name}" }
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
                                button {
                                    r#type: "button", class: "shelf-menu-item",
                                    "data-testid": "shelf-rename",
                                    onclick: move |_| { menu_open.set(false); renaming.set(true); },
                                    "Rename"
                                }
                                button {
                                    r#type: "button", class: "shelf-menu-item",
                                    "data-testid": "shelf-toggle-visibility",
                                    onclick: on_toggle_vis,
                                    "Change visibility"
                                }
                                button {
                                    r#type: "button", class: "shelf-menu-item shelf-menu-item--danger",
                                    "data-testid": "shelf-delete",
                                    onclick: on_delete,
                                    "Delete"
                                }
                            }
                        }
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
    let mut library = use_signal(Vec::<EbookMetadata>::new);
    let mut query = use_signal(String::new);
    let picked = use_signal(Vec::<String>::new);
    let mut saving = use_signal(|| false);

    let effect_url = server_url.clone();
    use_effect(move || {
        let url = effect_url.clone();
        spawn(async move {
            if let Ok(lib) = data::get_ebooks(&url).await {
                library.set(lib.books);
            }
        });
    });

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

    let q = query.read().to_lowercase();
    let books = library.read();
    let filtered: Vec<&EbookMetadata> = books
        .iter()
        .filter(|b| {
            q.is_empty()
                || b.title
                    .as_deref()
                    .unwrap_or(&b.filename)
                    .to_lowercase()
                    .contains(&q)
        })
        .collect();
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
                    div { class: "shelf-picker-grid",
                        for book in filtered.iter().map(|b| (*b).clone()) {
                            {add_picker_tile(book, &server_url, picked)}
                        }
                    }
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

/// One selectable tile in the add-books picker.
fn add_picker_tile(book: EbookMetadata, server_url: &str, picked: Signal<Vec<String>>) -> Element {
    let mut picked = picked;
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let selected = picked.read().contains(&uuid);
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);
    let toggle_uuid = uuid.clone();
    let class = if selected {
        "shelf-cover-tile shelf-cover-tile--selectable shelf-cover-tile--picked"
    } else {
        "shelf-cover-tile shelf-cover-tile--selectable"
    };

    rsx! {
        button {
            r#type: "button",
            class: "{class}",
            "aria-pressed": if selected { "true" } else { "false" },
            onclick: move |_| {
                let u = toggle_uuid.clone();
                picked.with_mut(|v| {
                    if let Some(pos) = v.iter().position(|x| x == &u) {
                        v.remove(pos);
                    } else {
                        v.push(u);
                    }
                });
            },
            Cover {
                book,
                src_override: src,
                srcset,
                sizes: Some("120px".to_string()),
            }
        }
    }
}

/// Build the responsive thumbnail `src`/`srcset` for a book (mirrors GridTile).
fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
        let base = format!("{server_url}/api/thumbs/{uuid}");
        (
            Some(format!("{base}/md")),
            Some(format!("{base}/sm 160w, {base}/md 320w, {base}/lg 640w")),
        )
    } else {
        (None, None)
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
