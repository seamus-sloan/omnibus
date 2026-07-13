//! Mobile shelf-detail surface — the phone counterpart to the web rail +
//! header + grid, matching the imported design. Mirrors `landing::mobile`: a
//! persistent "Omnibus" + search header, a title block with an eyebrow + facet
//! row (kind / visibility / rule chips), the shared Shelves entry card, a
//! three-column cover grid, and a ⋯ actions menu (rename / change visibility /
//! delete, + Add books on manual). Reuses the parent module's `rule_text` and
//! `landing::mobile_cover_cell`; the parent resolves the shelf before rendering
//! this, so it takes a loaded [`Shelf`].

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};
use omnibus_shared::{EbookMetadata, Shelf, ShelfKind, UpdateShelfRequest, Visibility};

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
    /// Base server URL used to build thumbnail `src`/`srcset`.
    pub server_url: String,
    /// Opens the shared add-books modal (manual shelves).
    pub on_add: EventHandler<()>,
    /// Opens the shared rule-editor modal (smart shelves).
    pub on_edit_rules: EventHandler<()>,
    /// Fired after a rename / visibility change so the parent refetches.
    pub on_changed: EventHandler<()>,
}

/// Mobile shelf-detail surface. Fed the loaded shelf + member books from
/// [`super::ShelfDetailPage`] so the data path stays shared across targets.
#[component]
pub(super) fn MobileShelfDetail(props: MobileShelfDetailProps) -> Element {
    let MobileShelfDetailProps {
        shelf,
        books,
        server_url,
        on_add,
        on_edit_rules,
        on_changed,
    } = props;

    let is_smart = shelf.kind == ShelfKind::Smart;
    let accent = shelf
        .accent
        .clone()
        .unwrap_or_else(|| "var(--accent)".into());
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
        div {
            class: "m-lib m-shelf-detail",
            style: "--accent: {accent};",
            "data-testid": "shelf-detail",
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
                    MobileShelfActions { shelf: shelf.clone(), on_add, on_edit_rules, on_changed }
                }
            }

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
                            span { key: "{i}", class: "m-shelf-facet-chip", "{super::rule_text(rule)}" }
                        }
                    }
                }
                if let Some(desc) = shelf.description.as_ref() {
                    p { class: "m-shelf-desc", "{desc}" }
                }
            }

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

            div { class: "m-cover-grid m-shelf-grid", "data-testid": "shelf-grid", role: "list",
                for book in books.iter().cloned() {
                    {mobile_cover_cell(book, &server_url)}
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

/// The header actions button + dropdown menu (rename, change visibility,
/// delete, and — for smart shelves — edit rules, or — for manual shelves —
/// add books). Owns its own menu / rename state so [`MobileShelfDetail`]
/// stays a pure presentation shell.
#[component]
fn MobileShelfActions(
    shelf: Shelf,
    on_add: EventHandler<()>,
    on_edit_rules: EventHandler<()>,
    on_changed: EventHandler<()>,
) -> Element {
    let nav = use_navigator();
    let server_url = use_server_url();
    let mut menu_open = use_signal(|| false);
    let mut renaming = use_signal(|| false);
    let mut draft_name = use_signal(|| shelf.name.clone());

    let id = shelf.id;
    let is_manual = shelf.kind == ShelfKind::Manual;
    let is_smart = shelf.kind == ShelfKind::Smart;
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
        div { class: "m-shelf-menu-wrap",
            if renaming() {
                input {
                    r#type: "text",
                    class: "shelf-name-input",
                    "data-testid": "shelf-rename-input",
                    value: "{draft_name}",
                    oninput: move |e| draft_name.set(e.value()),
                }
                button {
                    r#type: "button",
                    class: "btn shelf-btn-primary",
                    "data-testid": "shelf-rename-save",
                    onclick: on_rename_save,
                    "Save"
                }
            } else {
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
                        if is_smart {
                            button {
                                r#type: "button",
                                class: "shelf-menu-item",
                                "data-testid": "shelf-edit-rules",
                                onclick: move |_| {
                                    menu_open.set(false);
                                    on_edit_rules.call(());
                                },
                                "Edit rules"
                            }
                        }
                        button {
                            r#type: "button",
                            class: "shelf-menu-item",
                            "data-testid": "shelf-rename",
                            onclick: move |_| {
                                menu_open.set(false);
                                renaming.set(true);
                            },
                            "Rename"
                        }
                        button {
                            r#type: "button",
                            class: "shelf-menu-item",
                            "data-testid": "shelf-toggle-visibility",
                            onclick: on_toggle_vis,
                            "Change visibility"
                        }
                        button {
                            r#type: "button",
                            class: "shelf-menu-item shelf-menu-item--danger",
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
