//! Tests for the Hardcover provider's `map_book` — chiefly that a malformed
//! or hyphenated candidate ISBN-13 from the GraphQL response is normalized
//! (or rejected) the same way the sibling `openlibrary`/`googlebooks`
//! providers treat their own candidate lists.

use super::*;

/// Build a minimal `BookRow` with one candidate edition, so each test only
/// has to say what it's varying.
fn book_row(title: &str, edition_isbn_13: Option<&str>) -> BookRow {
    BookRow {
        title: Some(title.to_string()),
        description: None,
        contributions: Vec::new(),
        book_series: Vec::new(),
        image: None,
        editions: vec![Edition {
            isbn_13: edition_isbn_13.map(|s| s.to_string()),
        }],
    }
}

#[test]
fn map_book_normalizes_a_hyphenated_edition_isbn_when_no_scanned_isbn_is_given() {
    let row = book_row("Test Book", Some("978-0-13-468599-1"));

    let meta = map_book(row, None).expect("a valid, hyphenated isbn13 should still map");

    assert_eq!(meta.isbn13, "9780134685991");
}

#[test]
fn map_book_falls_through_to_none_when_the_only_edition_isbn_is_malformed() {
    let row = book_row("Test Book", Some("not-an-isbn"));

    assert!(
        map_book(row, None).is_none(),
        "a malformed edition isbn must not be passed through unnormalized"
    );
}

#[test]
fn map_book_falls_through_to_none_when_the_only_edition_isbn_has_a_bad_check_digit() {
    // Same digits as the valid fixture ISBN with the final check digit
    // flipped, so length and character-class checks alone wouldn't catch it.
    let row = book_row("Test Book", Some("9780134685990"));

    assert!(map_book(row, None).is_none());
}

#[test]
fn map_book_falls_through_to_none_when_no_edition_has_an_isbn() {
    let row = book_row("Test Book", None);

    assert!(map_book(row, None).is_none());
}

#[test]
fn map_book_uses_the_scanned_isbn_over_a_malformed_edition_isbn() {
    let row = book_row("Test Book", Some("not-an-isbn"));

    let meta = map_book(row, Some("9780134685991"))
        .expect("the scanned isbn is authoritative and needs no edition fallback");

    assert_eq!(meta.isbn13, "9780134685991");
}

#[test]
fn map_book_returns_none_when_the_title_is_missing() {
    let mut row = book_row("Test Book", Some("9780134685991"));
    row.title = None;

    assert!(map_book(row, None).is_none());
}
