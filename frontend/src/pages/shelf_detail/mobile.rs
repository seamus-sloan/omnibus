//! Mobile shelf-detail surface — the phone counterpart to the web rail, header,
//! and grid. Mirrors `landing::mobile`: a persistent header, a title block with
//! eyebrow and facet row (kind / visibility / rule chips), the shared Shelves
//! entry card, a three-column cover grid, and a ⋯ actions menu. The parent
//! resolves the shelf first, so this takes a loaded [`Shelf`].

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{EbookMetadata, Shelf, ShelfKind, Visibility};

use crate::components::shelf_facets::rule_text;
use crate::components::{confirm_modal_body, ConfirmModal, ConfirmModalAction, ConfirmModalTone};
use crate::pages::landing::mobile_cover_cell;
use crate::{data, use_server_url, Route};

/// Props for [`MobileShelfDetail`] — the loaded shelf + member books handed
/// down from [`super::ShelfDetailPage`] after its loading / not-found guards.
#[derive(Props, Clone, PartialEq)]
pub(super) struct MobileShelfDetailProps {
    /// The resolved shelf (parent guards loading / not-found).
    pub shelf: Shelf,
    /// The shelf's member books to render as cover cells.
    pub books: Vec<EbookMetadata>,
    /// Set when the member-books refetch failed, distinct from a genuinely
    /// empty shelf.
    pub errored: bool,
    /// Base server URL used to build thumbnail `src`/`srcset`.
    pub server_url: String,
    /// Opens the shared add-books modal (manual shelves).
    pub on_add: EventHandler<()>,
    /// Opens the shared edit-shelf modal (name / visibility / rules).
    pub on_edit: EventHandler<()>,
}

/// Mobile shelf-detail surface. Fed the loaded shelf + member books from
/// [`super::ShelfDetailPage`] so the data path stays shared across targets.
#[component]
pub(super) fn MobileShelfDetail(props: MobileShelfDetailProps) -> Element {
    let MobileShelfDetailProps {
        shelf,
        books,
        errored,
        server_url,
        on_add,
        on_edit,
    } = props;

    let is_smart = shelf.kind == ShelfKind::Smart;
    let accent = shelf
        .accent
        .clone()
        .unwrap_or_else(|| "var(--accent)".into());
    let cover_bust = crate::contexts::use_cover_cache_bust().0;

    rsx! {
        div {
            class: "m-lib m-shelf-detail",
            style: "--accent: {accent};",
            "data-testid": "shelf-detail",

            {mobile_shelf_head(&shelf, on_add, on_edit)}
            {mobile_shelf_title_block(&shelf, is_smart)}

            // Shelves entry — identical to the mobile landing card.
            Link {
                to: Route::Shelves {},
                class: "m-shelves-entry",
                "data-testid": "mobile-shelves-entry",
                span { class: "m-shelves-entry-icon", {bookmark_glyph()} }
                span { class: "m-shelves-entry-body",
                    span { class: "m-shelves-entry-name", "Shelves" }
                    span { class: "m-shelves-entry-sub", "Smart & hand-picked collections" }
                }
                span { class: "m-shelves-entry-chevron", {chevron()} }
            }

            if errored {
                p {
                    role: "alert",
                    class: "error",
                    "data-testid": "shelf-refetch-error",
                    "Couldn\u{2019}t refresh this shelf. Check your connection and try again."
                }
            }

            div { class: "m-cover-grid m-shelf-grid", "data-testid": "shelf-grid", role: "list",
                for book in books.iter().cloned() {
                    {mobile_cover_cell(book, &server_url, cover_bust)}
                }
            }
            if !is_smart {
                button {
                    r#type: "button",
                    class: "btn m-shelf-add",
                    "data-testid": "shelf-add-books",
                    onclick: move |_| on_add.call(()),
                    "\u{FF0B} Add books"
                }
            }
        }
    }
}

/// Persistent header: brand word, search entry, and — for non-system
/// shelves — the actions menu.
fn mobile_shelf_head(
    shelf: &Shelf,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
) -> Element {
    rsx! {
        header { class: "m-lib-head",
            div { class: "omn-brand-word m-lib-brand", "Omnibus" }
            div { class: "m-lib-head-actions",
                Link {
                    to: Route::MobileSearch {},
                    class: "m-icon-btn",
                    "aria-label": "Search",
                    "data-testid": "mobile-search-entry",
                    {search_glyph()}
                }
                // System shelves (Wishlist) are locked server-side — don't
                // offer edit/delete the server would reject (mirrors web's
                // `can_manage` gate).
                if !shelf.kind.is_system() {
                    MobileShelfActions { shelf: shelf.clone(), on_add, on_edit }
                }
            }
        }
    }
}

/// Title block: eyebrow (count + auto-filled/hand-picked), name, the
/// kind/visibility/rule-chip facet row, and an optional description.
fn mobile_shelf_title_block(shelf: &Shelf, is_smart: bool) -> Element {
    let kicker = format!(
        "{} {} \u{00b7} {}",
        shelf.book_count,
        if shelf.book_count == 1 {
            "book"
        } else {
            "books"
        },
        if is_smart {
            "auto-filled"
        } else {
            "hand-picked"
        },
    );
    let kind_label = if is_smart { "Smart" } else { "Hand-picked" };
    let vis_label = match shelf.visibility {
        Visibility::Private => "Private",
        Visibility::Public => "Shared",
    };
    rsx! {
        div { class: "m-lib-title",
            span { class: "label", "{kicker}" }
            h2 { class: "m-head-title", span { class: "m-em", "{shelf.name}" } }
            div { class: "m-shelf-facets",
                span { class: "m-shelf-facet-kind",
                    if is_smart {
                        {smart_facet_glyph()}
                    }
                    "{kind_label}"
                }
                span { class: "m-shelf-facet-dot", "\u{00b7}" }
                span { class: "m-shelf-facet-vis", "{vis_label}" }
                if is_smart {
                    for (i, rule) in shelf.rules.iter().enumerate() {
                        span { key: "{i}", class: "m-shelf-facet-chip", "{rule_text(rule)}" }
                    }
                }
            }
            if let Some(desc) = shelf.description.as_ref() {
                p { class: "m-shelf-desc", "{desc}" }
            }
        }
    }
}

/// The header actions button + dropdown menu (edit shelf, delete, and — for
/// manual shelves — add books). Name / visibility / rule edits all live in
/// the shared edit-shelf modal opened via `on_edit`. Owns its own menu state
/// so [`MobileShelfDetail`] stays a pure presentation shell.
#[component]
fn MobileShelfActions(
    shelf: Shelf,
    on_add: EventHandler<()>,
    on_edit: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let mut menu_open = use_signal(|| false);
    let mut show_delete_confirm = use_signal(|| false);
    let deleting = use_signal(|| false);

    let id = shelf.id;
    let shelf_name = shelf.name.clone();
    let is_manual = shelf.kind == ShelfKind::Manual;

    rsx! {
        div { class: "m-shelf-menu-wrap",
            button {
                r#type: "button",
                class: "m-icon-btn",
                "data-testid": "shelf-actions",
                "aria-label": "Shelf actions",
                onclick: move |_| menu_open.toggle(),
                "\u{22EF}"
            }
            if menu_open() {
                div { class: "shelf-actions-menu",
                    if is_manual {
                        button {
                            r#type: "button",
                            class: "shelf-menu-item",
                            "data-testid": "shelf-add-books-menu",
                            onclick: move |_| {
                                menu_open.set(false);
                                on_add.call(());
                            },
                            "Add books"
                        }
                    }
                    button {
                        r#type: "button",
                        class: "shelf-menu-item",
                        "data-testid": "shelf-edit",
                        onclick: move |_| {
                            menu_open.set(false);
                            on_edit.call(());
                        },
                        "Edit shelf"
                    }
                    button {
                        r#type: "button",
                        class: "shelf-menu-item shelf-menu-item--danger",
                        "data-testid": "shelf-delete",
                        onclick: move |_| {
                            menu_open.set(false);
                            show_delete_confirm.set(true);
                        },
                        "Delete"
                    }
                }
            }
        }
        if show_delete_confirm() {
            {render_delete_shelf_modal(
                server_url,
                id,
                shelf_name,
                nav,
                show_delete_confirm,
                deleting,
            )}
        }
    }
}

/// The delete-shelf confirm modal: names the shelf, disables the confirm
/// button while the request is in flight, and can't be dismissed mid-delete.
/// On success, navigates back to the library. Mirrors the web header's
/// version of the same modal (`shelf_detail::header::render_delete_shelf_modal`).
fn render_delete_shelf_modal(
    server_url: String,
    id: i64,
    shelf_name: String,
    nav: dioxus_router::Navigator,
    mut show_delete_confirm: Signal<bool>,
    mut deleting: Signal<bool>,
) -> Element {
    let is_busy = deleting();
    let do_delete = move |_| {
        if deleting() {
            return;
        }
        deleting.set(true);
        let url = server_url.clone();
        spawn(async move {
            if data::delete_shelf(&url, id).await.is_ok() {
                nav.push(Route::Landing {});
            } else {
                deleting.set(false);
            }
        });
    };
    rsx! {
        ConfirmModal {
            testid: "shelf-delete-modal".to_string(),
            aria_label: "Delete shelf?".to_string(),
            dialog_class: "mg-modal del-modal".to_string(),
            busy: is_busy,
            on_dismiss: move |_| show_delete_confirm.set(false),
            {confirm_modal_body(
                "Delete shelf?",
                &format!(
                    "Deleting \u{201c}{shelf_name}\u{201d} removes it and its rules. This can\u{2019}t be undone."
                ),
                vec![
                    ConfirmModalAction {
                        testid: "shelf-delete-cancel".to_string(),
                        label: "Cancel".to_string(),
                        tone: ConfirmModalTone::Ghost,
                        disabled: is_busy,
                        on_click: EventHandler::new(move |_| show_delete_confirm.set(false)),
                    },
                    ConfirmModalAction {
                        testid: "shelf-delete-confirm".to_string(),
                        label: if is_busy { "Deleting\u{2026}".to_string() } else { "Delete".to_string() },
                        tone: ConfirmModalTone::Danger,
                        disabled: is_busy,
                        on_click: EventHandler::new(do_delete),
                    },
                ],
            )}
        }
    }
}

fn search_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}

fn bookmark_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" }
        }
    }
}

fn chevron() -> Element {
    rsx! {
        svg {
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M9 18l6-6-6-6" }
        }
    }
}

/// Small accent cog rendered before "Smart" in the facet row.
fn smart_facet_glyph() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M5 3v4M3 5h4M6 17v4M4 19h4" }
            path { d: "M13 3l2.5 6.5L22 12l-6.5 2.5L13 21l-2.5-6.5L4 12l6.5-2.5z" }
        }
    }
}
