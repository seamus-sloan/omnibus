//! Power-user table view for the landing page.
//!
//! Dense `<table>` with inline-editable title / series / authors / tags /
//! genres for
//! admins, used by [`super::LandingPage`] when the view-mode toggle is set to
//! table.
//! Row plumbing in [`row`]; per-cell rendering in [`cells`].

mod cells;
mod row;

use std::collections::BTreeSet;

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, SortDir, SortKey, ViewPrefs};

use row::EbookRow;

use crate::components::chip_editor::SuggestionItem;

/// Inline-editable field on a power-user-table row. Used by `EbookRow` to
/// track which of its cells (if any) is in edit mode at any given time —
/// single-cell-at-a-time keeps the keyboard/blur lifecycle simple and
/// avoids fighting the row's click-to-navigate handler.
///
/// `Series` edits the series *name* only — the series index ("#N") stays
/// in the full metadata edit page on `/books/:uuid/edit` to avoid adding a
/// separate Series Index column to the power-user table. Title + Series +
/// Authors + Tags + Genres covers the stated cleanup pain (stripping
/// series-prefix cruft from titles, renaming author/series variants,
/// curating tags, assigning genres).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditField {
    Title,
    Series,
    Tags,
    Genres,
    Published,
    Language,
    Authors,
}

/// Per-page context threaded through `BookTable` → `EbookRow` → cells.
#[derive(Clone, PartialEq)]
pub(super) struct BookTableContext {
    pub(super) server_url: String,
    pub(super) is_admin: bool,
    pub(super) author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub(super) tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub(super) genre_suggestions: ReadSignal<Vec<SuggestionItem>>,
    /// Bulk-edit selection: the uuids of the checked rows. Owned by the
    /// landing page so the floating action bar can read it too.
    pub(super) selected: Signal<BTreeSet<String>>,
}

/// Power-user table view of `books`: sortable column headers and inline-editable cells.
#[component]
pub(super) fn BookTable(
    books: Vec<EbookMetadata>,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
    ctx: BookTableContext,
) -> Element {
    // Snapshot the visible uuids before `books` is consumed by the row loop —
    // select-all operates on exactly the rows this render shows.
    let visible_uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let mut selected = ctx.selected;
    let all_selected = !visible_uuids.is_empty() && {
        let set = selected.read();
        visible_uuids.iter().all(|u| set.contains(u))
    };
    rsx! {
        div {
            id: "ebook-table",
            "data-testid": "ebook-table",
            class: "ebook-table-wrap",
            table { class: "ebook-table",
                thead {
                    tr {
                        if ctx.is_admin {
                            th { class: "ebook-col-select",
                                input {
                                    r#type: "checkbox",
                                    "data-testid": "ebook-select-all",
                                    aria_label: "Select all books",
                                    checked: all_selected,
                                    onchange: move |_| {
                                        let mut set = selected.write();
                                        if all_selected {
                                            for uuid in &visible_uuids {
                                                set.remove(uuid);
                                            }
                                        } else {
                                            set.extend(visible_uuids.iter().cloned());
                                        }
                                    },
                                }
                            }
                        }
                        th { class: "ebook-col-cover", "Cover" }
                        SortableHeader {
                            class: "ebook-col-title".to_string(),
                            label: "Title".to_string(),
                            sort_key: SortKey::Title,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-author".to_string(),
                            label: "Author".to_string(),
                            sort_key: SortKey::Author,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-series".to_string(),
                            label: "Series".to_string(),
                            sort_key: SortKey::Series,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-tags", "Tags" }
                        th { class: "ebook-col-genres", "Genres" }
                        th { class: "ebook-col-published", "Published" }
                        th { class: "ebook-col-formats", "Formats" }
                        SortableHeader {
                            class: "ebook-col-updated".to_string(),
                            label: "Last Updated".to_string(),
                            sort_key: SortKey::LastUpdated,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        SortableHeader {
                            class: "ebook-col-added".to_string(),
                            label: "Added".to_string(),
                            sort_key: SortKey::NewestAdded,
                            prefs: prefs.clone(),
                            on_sort: on_sort,
                        }
                        th { class: "ebook-col-language", "Language" }
                    }
                }
                tbody {
                    for book in books.into_iter() {
                        EbookRow {
                            key: "{super::sorting::row_ident(&book)}",
                            book: book,
                            ctx: ctx.clone(),
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn SortableHeader(
    class: String,
    label: String,
    sort_key: SortKey,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
) -> Element {
    let active = prefs.sort_key == sort_key;
    let aria_sort = match (active, prefs.sort_dir) {
        (true, SortDir::Asc) => "ascending",
        (true, SortDir::Desc) => "descending",
        _ => "none",
    };
    let arrow = if !active {
        ""
    } else if prefs.sort_dir == SortDir::Asc {
        " ↑"
    } else {
        " ↓"
    };
    rsx! {
        th { class: "{class} sort-th", aria_sort: "{aria_sort}",
            button {
                class: "sort-th-btn",
                onclick: move |_| on_sort.call(sort_key),
                "{label}{arrow}"
            }
        }
    }
}
