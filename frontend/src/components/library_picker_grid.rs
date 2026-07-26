//! Shared "pick books from the whole library" primitives: fetch-on-mount,
//! substring filter, and the selectable cover grid. Used by the create-shelf
//! modal's hand-picked body and the shelf-detail "add books" modal so both
//! pickers share one implementation instead of two independently-written
//! copies of the same fetch/filter/grid shape.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use crate::components::cover_tile::toggle_picked;
use crate::components::{CoverTile, CoverTileKind};
use crate::data;

/// Fetches the full library once on mount, for a picker's search/select UI.
pub fn use_library_fetch(server_url: String, mut library: Signal<Vec<EbookMetadata>>) {
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
pub fn filter_library<'a>(books: &'a [EbookMetadata], query: &str) -> Vec<&'a EbookMetadata> {
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

/// Selectable cover grid over a (typically already-filtered) set of library
/// books; toggles membership in `picked` via `CoverTile`'s `Selectable` kind.
#[component]
pub fn LibraryPickerGrid(
    books: Vec<EbookMetadata>,
    server_url: String,
    picked: Signal<Vec<String>>,
) -> Element {
    rsx! {
        div { class: "shelf-picker-grid",
            for book in books {
                {
                    let uuid = book.unique_identifier.clone().unwrap_or_default();
                    let selected = picked.read().contains(&uuid);
                    let mut picked = picked;
                    rsx! {
                        div {
                            key: "{book.id}",
                            CoverTile {
                                book,
                                server_url: server_url.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn book(title: Option<&str>, filename: &str) -> EbookMetadata {
        EbookMetadata {
            title: title.map(str::to_string),
            filename: filename.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn filter_library_matches_title_case_insensitively() {
        let books = vec![book(Some("The Great Gatsby"), "gatsby.epub")];
        assert_eq!(filter_library(&books, "great").len(), 1);
        assert_eq!(filter_library(&books, "GATSBY").len(), 1);
        assert_eq!(filter_library(&books, "moby").len(), 0);
    }

    #[test]
    fn filter_library_falls_back_to_filename_when_title_is_missing() {
        let books = vec![book(None, "untitled-scan.epub")];
        assert_eq!(filter_library(&books, "untitled").len(), 1);
    }

    #[test]
    fn filter_library_returns_every_book_when_query_is_empty() {
        let books = vec![book(Some("A"), "a.epub"), book(Some("B"), "b.epub")];
        assert_eq!(filter_library(&books, "").len(), 2);
    }
}
