//! URL-derivation tests for `BookSuggestion::new`.

use super::*;

/// A `RawSuggestion` with the given id and slug (`None`/empty exercises the
/// id-fallback branch); title, author, and list_count are fixed.
fn raw(hardcover_id: i64, hardcover_slug: Option<String>) -> RawSuggestion {
    RawSuggestion {
        hardcover_id,
        hardcover_slug,
        title: "Dune".into(),
        author: "Frank Herbert".into(),
        list_count: 7,
    }
}

#[test]
fn new_uses_slug_url_and_derives_cover_url_when_cover_cached() {
    let s = BookSuggestion::new("book-1", 2, raw(42, Some("dune".into())), true);
    assert_eq!(s.hardcover_id, 42);
    assert_eq!(s.hardcover_url, "https://hardcover.app/books/dune");
    assert_eq!(s.title, "Dune");
    assert_eq!(s.author, "Frank Herbert");
    assert_eq!(s.list_count, 7);
    assert_eq!(
        s.cover_url.as_deref(),
        Some("/api/suggestions/book-1/2/cover")
    );
}

#[test]
fn new_falls_back_to_id_url_when_slug_is_none() {
    let s = BookSuggestion::new("book-1", 0, raw(99, None), false);
    assert_eq!(s.hardcover_url, "https://hardcover.app/books/99");
}

#[test]
fn new_falls_back_to_id_url_when_slug_is_empty() {
    let s = BookSuggestion::new("book-1", 0, raw(99, Some(String::new())), false);
    assert_eq!(s.hardcover_url, "https://hardcover.app/books/99");
}

#[test]
fn new_omits_cover_url_when_no_cover_cached() {
    let s = BookSuggestion::new("book-1", 3, raw(42, Some("dune".into())), false);
    assert_eq!(s.cover_url, None);
}
