//! Web shelf page: the way back to the index, the hero (see
//! [`super::header`]), and the member books — a sort control on smart
//! shelves, the cover grid with a remove control on each book the viewer may
//! take off, and empty states that say what fills this kind of shelf. Draws
//! only once the viewer is known, so the actions never render enabled and then
//! grey out.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, Shelf, ShelfKind, SortKey};

use super::header::ShelfHero;
use crate::components::atrium::fallback_title;
use crate::components::shelf_glyphs::{plus_icon, x_icon};
use crate::components::{CoverTile, CoverTileKind, PageLoading};
use crate::shelf_access::ShelfAccess;
use crate::{data, Route};

/// UI-state signals the page owns and this surface drives. `Copy` (Dioxus
/// signals), so grouping them keeps the props list short.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct ShelfBodySignals {
    pub sort_key: Signal<SortKey>,
    pub show_add: Signal<bool>,
    pub edit_shelf: Signal<bool>,
    pub reload: Signal<u32>,
}

/// The web shelf page — see the module doc.
#[component]
pub(super) fn WebShelfBody(
    shelf: Shelf,
    books: Vec<EbookMetadata>,
    errored: bool,
    server_url: String,
    signals: ShelfBodySignals,
) -> Element {
    let ShelfBodySignals {
        sort_key,
        mut show_add,
        mut edit_shelf,
        mut reload,
    } = signals;
    let viewer_probe = crate::use_current_user().0;
    // Books whose removal is in flight, or has landed but not yet been
    // refetched away: dimmed and inert until the next member list arrives.
    let mut leaving = use_signal(Vec::<String>::new);
    let mut remove_error = use_signal(|| None::<String>);
    use_effect(use_reactive!(|books| {
        let _ = books;
        if !leaving.peek().is_empty() {
            leaving.set(Vec::new());
        }
    }));

    let Some(viewer) = viewer_probe() else {
        return rsx! {
            div { class: "shd-page",
                {back_crumb()}
                div { class: "shd-state", PageLoading {} }
            }
        };
    };
    let access = ShelfAccess::resolve(
        viewer.as_ref(),
        shelf.owner_user_id,
        &shelf.owner_username,
        shelf.kind,
    );
    let accent = shelf
        .accent
        .clone()
        .unwrap_or_else(|| "var(--accent)".into());
    let shelf_id = shelf.id;
    let remove_url = server_url.clone();
    let on_remove = EventHandler::new(move |(uuid, title): (String, String)| {
        if leaving.peek().contains(&uuid) {
            return;
        }
        leaving.write().push(uuid.clone());
        remove_error.set(None);
        let url = remove_url.clone();
        spawn(async move {
            match data::remove_shelf_book(&url, shelf_id, &uuid).await {
                Ok(()) => reload.with_mut(|n| *n += 1),
                Err(e) => {
                    leaving.write().retain(|u| *u != uuid);
                    remove_error.set(Some(format!(
                        "Couldn\u{2019}t remove \u{201c}{title}\u{201d} from this shelf: {e}"
                    )));
                }
            }
        });
    });
    let view = BooksView {
        kind: shelf.kind,
        books,
        errored,
        server_url,
        can_edit: access.can_edit(),
        leaving: leaving(),
        remove_error: remove_error(),
    };

    rsx! {
        div {
            class: "shd-page",
            style: "--shelf-accent: {accent};",
            "data-testid": "shelf-detail",
            {back_crumb()}
            ShelfHero {
                shelf: shelf.clone(),
                access,
                on_add: move |_| show_add.set(true),
                on_edit: move |_| edit_shelf.set(true),
                on_changed: move |_| reload.with_mut(|n| *n += 1),
            }
            ShelfBooks {
                view,
                sort_key,
                on_add: move |_| show_add.set(true),
                on_edit: move |_| edit_shelf.set(true),
                on_remove,
            }
        }
    }
}

/// The way back to the index, above every state of the page.
pub(super) fn back_crumb() -> Element {
    rsx! {
        nav { class: "shd-crumbs", "aria-label": "Breadcrumb",
            Link { to: Route::Shelves {}, class: "shd-back", "data-testid": "shelf-back",
                "\u{2190} All shelves"
            }
        }
    }
}

/// What the member section renders from.
#[derive(Clone, PartialEq)]
struct BooksView {
    kind: ShelfKind,
    books: Vec<EbookMetadata>,
    errored: bool,
    server_url: String,
    /// The viewer may change this shelf's membership or rules.
    can_edit: bool,
    /// Uuids of books being taken off the shelf.
    leaving: Vec<String>,
    remove_error: Option<String>,
}

/// The member books: heading and count, the smart-shelf sort, any fetch or
/// removal failure, then the grid — or an empty state in the kind's terms.
#[component]
fn ShelfBooks(
    view: BooksView,
    sort_key: Signal<SortKey>,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
    on_remove: EventHandler<(String, String)>,
) -> Element {
    let removable = view.can_edit && view.kind == ShelfKind::Manual;
    let count = view.books.len();
    rsx! {
        section { class: "shd-books", "aria-labelledby": "shd-books-title",
            div { class: "shd-books-head",
                h2 { class: "shd-books-title", id: "shd-books-title",
                    "Books"
                    span { class: "mono shd-books-count", "{count}" }
                }
                if view.kind == ShelfKind::Smart {
                    {sort_control(sort_key)}
                }
            }
            if view.errored {
                p { role: "alert", class: "error shd-alert", "data-testid": "shelf-refetch-error",
                    "Couldn\u{2019}t refresh this shelf. Check your connection and try again."
                }
            }
            if let Some(msg) = view.remove_error.clone() {
                p { role: "alert", class: "error shd-alert", "data-testid": "shelf-remove-error", "{msg}" }
            }
            if view.books.is_empty() && !view.errored {
                {empty_state(view.kind, view.can_edit, on_add, on_edit)}
            } else {
                div { class: "lib-grid shelf-grid shd-grid", "data-testid": "shelf-grid", role: "list",
                    for book in view.books.iter().cloned() {
                        {member_tile(book, &view.server_url, removable, &view.leaving, on_remove)}
                    }
                    if removable {
                        button {
                            r#type: "button",
                            class: "shelf-add-tile",
                            "data-testid": "shelf-add-tile",
                            onclick: move |_| on_add.call(()),
                            span { class: "shelf-add-tile-plus", {plus_icon()} }
                            span { "Add books" }
                        }
                    }
                }
            }
        }
    }
}

/// Sort for a smart shelf — the only kind whose order the server computes.
fn sort_control(mut sort_key: Signal<SortKey>) -> Element {
    rsx! {
        label { class: "shd-sort",
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
                option { value: "recently_interacted", "Recently read" }
                option { value: "title", "Title" }
                option { value: "author", "Author" }
                option { value: "series", "Series" }
                option { value: "newest_added", "Newest added" }
                option { value: "last_updated", "Recently updated" }
            }
        }
    }
}

/// One member: its cover tile, plus — when the viewer may take it off — a
/// remove control that names the book it removes.
fn member_tile(
    book: EbookMetadata,
    server_url: &str,
    removable: bool,
    leaving: &[String],
    on_remove: EventHandler<(String, String)>,
) -> Element {
    // Match `Cover`'s empty-title handling (issue #92): blank titles fall back
    // to the filename so the caption and both labels never render empty.
    let title = fallback_title(book.title.as_deref(), &book.filename);
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let is_leaving = leaving.contains(&uuid);
    let key = book.id;
    let remove_label = format!("Remove {title} from this shelf");
    rsx! {
        div {
            key: "{key}",
            class: if is_leaving { "shd-tile shd-tile--leaving" } else { "shd-tile" },
            CoverTile {
                book,
                server_url: server_url.to_string(),
                sizes: "(max-width: 640px) 160px, 200px".to_string(),
                kind: CoverTileKind::MemberLink { title: title.clone() },
            }
            if removable {
                button {
                    r#type: "button",
                    class: "shd-tile-remove",
                    "data-testid": "shelf-remove-{uuid}",
                    "aria-label": "{remove_label}",
                    title: "Remove from shelf",
                    disabled: is_leaving,
                    onclick: move |_| on_remove.call((uuid.clone(), title.clone())),
                    {x_icon()}
                }
            }
        }
    }
}

/// What an empty shelf says, in its kind's terms, with the one action that
/// would fill it when the viewer may take it.
fn empty_state(
    kind: ShelfKind,
    can_edit: bool,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
) -> Element {
    let message = match kind {
        ShelfKind::Manual => "No books on this shelf yet.",
        ShelfKind::Smart => "No books match this shelf\u{2019}s rules yet.",
        ShelfKind::Wishlist => "Nothing on this wishlist yet.",
    };
    rsx! {
        div { class: "shd-empty", "data-testid": "shelf-empty",
            p { class: "shd-empty-msg", "{message}" }
            if can_edit && kind == ShelfKind::Manual {
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "shelf-empty-add",
                    onclick: move |_| on_add.call(()),
                    {plus_icon()}
                    "Add books"
                }
            } else if can_edit && kind == ShelfKind::Smart {
                button {
                    r#type: "button",
                    class: "btn",
                    "data-testid": "shelf-empty-edit",
                    onclick: move |_| on_edit.call(()),
                    "Edit rules"
                }
            }
        }
    }
}
