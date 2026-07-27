//! Web presentation for the shelf detail page: rail + header, the smart-shelf
//! rule chips / sort row, and the member-books grid with its "Add books"
//! tile. Mobile renders its own surface in `super::mobile` instead.

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, Shelf, ShelfKind, SortKey};

use super::header::ShelfHeader;
use super::rule_text;
use crate::components::atrium::fallback_title;
use crate::components::{CoverTile, CoverTileKind, RailActive, ShelvesRail};

/// UI-state signals threaded into [`web_shelf_body`]. `Copy` (Dioxus
/// signals), so grouping them keeps the function under clippy's
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
pub(super) struct ShelfBodySignals {
    pub sort_key: Signal<SortKey>,
    pub show_add: Signal<bool>,
    pub edit_rules: Signal<bool>,
    pub reload: Signal<u32>,
}

/// Web presentation: rail + header (badges/actions), rule chips + sort row
/// for smart shelves, and the `CoverTile` member grid.
pub(super) fn web_shelf_body(
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

                {
                    // Only hand-picked (manual) shelves get the "Add books"
                    // tile — smart membership is computed and wishlist
                    // membership is derived from `wishlist_entries`.
                    let allow_add = current.kind == ShelfKind::Manual;
                    member_grid(books, server_url, allow_add, move |_| show_add.set(true))
                }
            }
        }
    }
}

/// Member-books grid; manual shelves get a trailing dashed "Add books" tile.
fn member_grid(
    books: &[EbookMetadata],
    server_url: &str,
    allow_add: bool,
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
            if allow_add {
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
