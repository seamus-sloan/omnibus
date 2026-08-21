//! Tests for the structured query: what counts as absent, and what each
//! widening step drops.

use super::*;

#[test]
fn new_trims_and_treats_blank_as_absent() {
    let q = SearchQuery::new(Some("  Dune  "), Some("   "), None);
    assert_eq!(q.title.as_deref(), Some("Dune"));
    assert_eq!(q.author, None);
    assert_eq!(q.isbn13, None);
}

#[test]
fn new_normalizes_a_usable_isbn() {
    let q = SearchQuery::new(None, None, Some("978-0-13-468599-1"));
    assert_eq!(q.isbn13.as_deref(), Some("9780134685991"));
}

#[test]
fn new_converts_an_isbn_10_to_its_isbn_13() {
    let q = SearchQuery::new(None, None, Some("0134685997"));
    assert_eq!(q.isbn13.as_deref(), Some("9780134685991"));
}

#[test]
fn new_discards_a_malformed_isbn_rather_than_failing() {
    // The ISBN is a hint here, not the thing being looked up: a stale form
    // field holding a typo must not refuse the whole search.
    let q = SearchQuery::new(Some("Dune"), None, Some("not-an-isbn"));
    assert_eq!(q.isbn13, None);
    assert_eq!(q.title.as_deref(), Some("Dune"));
}

#[test]
fn from_text_reads_free_text_as_a_title_only() {
    let q = SearchQuery::from_text("  dune messiah ");
    assert_eq!(q.title.as_deref(), Some("dune messiah"));
    assert_eq!(q.author, None);
    assert_eq!(q.isbn13, None);
}

#[test]
fn without_isbn_keeps_the_terms_and_drops_the_identifier() {
    let q = SearchQuery::new(Some("Dune"), Some("Frank Herbert"), Some("9780134685991"));
    let widened = q.without_isbn();
    assert_eq!(widened.isbn13, None);
    assert_eq!(widened.title.as_deref(), Some("Dune"));
    assert_eq!(widened.author.as_deref(), Some("Frank Herbert"));
}

#[test]
fn without_author_drops_the_author_and_the_identifier() {
    let q = SearchQuery::new(Some("Dune"), Some("Frank Herbert"), Some("9780134685991"));
    let widened = q.without_author();
    assert_eq!(widened.author, None);
    assert_eq!(widened.isbn13, None);
    assert_eq!(widened.title.as_deref(), Some("Dune"));
}

#[test]
fn as_text_joins_title_and_author_for_a_free_text_provider() {
    assert_eq!(
        SearchQuery::new(Some("Dune"), Some("Frank Herbert"), None).as_text(),
        "Dune Frank Herbert"
    );
    assert_eq!(SearchQuery::new(Some("Dune"), None, None).as_text(), "Dune");
    assert_eq!(
        SearchQuery::new(None, Some("Frank Herbert"), None).as_text(),
        "Frank Herbert"
    );
    assert_eq!(SearchQuery::default().as_text(), "");
}

#[test]
fn is_empty_reports_a_query_with_nothing_to_ask() {
    assert!(SearchQuery::default().is_empty());
    assert!(SearchQuery::from_text("   ").is_empty());
    assert!(!SearchQuery::from_text("Dune").is_empty());
}
