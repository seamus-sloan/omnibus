//! Power-user table view for the landing page.
//!
//! Dense `<table>` rendering with inline-editable authors / title / tags
//! for admins. Used by [`super::LandingPage`] when the view-mode toggle is
//! set to table. Per-row state plumbing lives in [`row`]; per-cell
//! renderers and save-callback factories live in [`cells`].

mod cells;
mod row;

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
/// Authors covers the stated cleanup pain (stripping series-prefix cruft
/// from titles, renaming author/series variants).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditField {
    Title,
    Series,
    Publisher,
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
}

#[component]
pub(super) fn BookTable(
    books: Vec<EbookMetadata>,
    prefs: ViewPrefs,
    on_sort: EventHandler<SortKey>,
    ctx: BookTableContext,
) -> Element {
    rsx! {
        div {
            id: "ebook-table",
            "data-testid": "ebook-table",
            class: "ebook-table-wrap",
            table { class: "ebook-table",
                thead {
                    tr {
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
                        th { class: "ebook-col-publisher", "Publisher" }
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
                            key: "{book.filename}",
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
