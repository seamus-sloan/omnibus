//! Pure replica pagination / sorting / search tests.

use omnibus_shared::Contributor;

use super::*;

fn book(title: &str, author: &str, formats: &[&str]) -> EbookMetadata {
    EbookMetadata {
        title: Some(title.to_string()),
        filename: format!("{title}.epub"),
        creators: vec![Contributor {
            name: author.to_string(),
            ..Default::default()
        }],
        formats: formats.iter().map(|f| f.to_string()).collect(),
        unique_identifier: Some(format!("uuid-{title}")),
        ..Default::default()
    }
}

fn titles(page: &LibraryPage) -> Vec<String> {
    page.books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect()
}

fn fixture() -> Vec<EbookMetadata> {
    vec![
        book("Cider House", "Irving", &["EPUB"]),
        book("Atonement", "McEwan", &["EPUB", "M4B"]),
        book("Beloved", "Morrison", &["M4B"]),
        book("Dune", "Herbert", &["EPUB"]),
    ]
}

#[test]
fn page_from_replica_sorts_by_title_and_slices_by_cursor() {
    let first = page_from_replica(fixture(), SortKey::Title, SortDir::Asc, &[], None, 2);
    assert_eq!(titles(&first), vec!["Atonement", "Beloved"]);
    assert_eq!(first.total, Some(4));
    assert_eq!(first.next_cursor.as_deref(), Some("off:2"));

    let second = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        Some("off:2"),
        2,
    );
    assert_eq!(titles(&second), vec!["Cider House", "Dune"]);
    // `total` is a first-page-only field, mirroring the online headers.
    assert_eq!(second.total, None);
    assert_eq!(second.next_cursor, None);
}

#[test]
fn page_from_replica_flips_direction() {
    let page = page_from_replica(fixture(), SortKey::Title, SortDir::Desc, &[], None, 10);
    assert_eq!(
        titles(&page),
        vec!["Dune", "Cider House", "Beloved", "Atonement"]
    );
}

#[test]
fn page_from_replica_sorts_by_author_with_title_tiebreak() {
    let mut books = fixture();
    books.push(book("A Widow for One Year", "Irving", &["EPUB"]));
    let page = page_from_replica(books, SortKey::Author, SortDir::Asc, &[], None, 10);
    assert_eq!(
        titles(&page),
        vec![
            "Dune",                 // Herbert
            "A Widow for One Year", // Irving (title tiebreak)
            "Cider House",          // Irving
            "Atonement",            // McEwan
            "Beloved",              // Morrison
        ]
    );
}

#[test]
fn page_from_replica_filters_by_format_any_match() {
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &["m4b".to_string()],
        None,
        10,
    );
    assert_eq!(titles(&page), vec!["Atonement", "Beloved"]);
    assert_eq!(page.total, Some(2));
}

#[test]
fn page_from_replica_ends_stream_on_foreign_cursor() {
    // A leftover *online* keyset cursor must not restart pagination from the
    // top (duplicate grid keys); it ends the stream instead.
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        Some("eyJvbmxpbmUiOiJjdXJzb3IifQ"),
        10,
    );
    assert!(page.books.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn page_from_replica_returns_empty_page_past_the_end() {
    let page = page_from_replica(
        fixture(),
        SortKey::Title,
        SortDir::Asc,
        &[],
        Some("off:99"),
        10,
    );
    assert!(page.books.is_empty());
    assert_eq!(page.next_cursor, None);
}

#[test]
fn search_replica_matches_title_author_and_filename_case_insensitively() {
    let books = fixture();
    let by_title = search_replica(&books, "dUnE");
    assert_eq!(by_title.len(), 1);
    assert_eq!(by_title[0].title.as_deref(), Some("Dune"));

    let by_author = search_replica(&books, "morrison");
    assert_eq!(by_author.len(), 1);
    assert_eq!(by_author[0].title.as_deref(), Some("Beloved"));

    // filename contains "Atonement.epub"
    let by_file = search_replica(&books, "atonement.epub");
    assert_eq!(by_file.len(), 1);

    assert!(search_replica(&books, "   ").is_empty());
    assert!(search_replica(&books, "zzz-no-hit").is_empty());
}
