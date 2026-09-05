//! `search_palette` end-to-end: each arm's substring match (books,
//! authors, series, tags), the facet matches on a book hit, the empty and
//! no-result cases, single-library scoping, the audiobook duration field,
//! and the two error conversions.

use super::super::*;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

// ── search_palette ───────────────────────────────────────────────
#[tokio::test]
async fn search_palette_books_match_title() {
    let _covers = CoversTempDir::new("palette_books");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &["Horror"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("Frankenstein"),
                &["Mary Shelley"],
                &["Horror"],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "dracula").await.unwrap();
    assert_eq!(results.books.len(), 1);
    assert_eq!(results.books[0].title, "Dracula");
    assert_eq!(results.books[0].author_display, "Bram Stoker");
    assert!(results.books[0].formats.contains(&"EPUB".to_string()));
}

#[tokio::test]
async fn search_palette_authors_match_substring() {
    let _covers = CoversTempDir::new("palette_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Babel"),
            &["R. F. Kuang"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "kuang").await.unwrap();
    assert!(!results.authors.is_empty(), "should match author substring");
    assert_eq!(results.authors[0].name, "R. F. Kuang");
    assert_eq!(results.authors[0].book_count, 1);
    // F1 results page: the "incl. <title>" line draws on the author's first
    // book in the library.
    assert_eq!(
        results.authors[0].lead_book_title.as_deref(),
        Some("Babel"),
        "author hit should carry its lead book title"
    );
}

#[tokio::test]
async fn search_palette_series_match() {
    let _covers = CoversTempDir::new("palette_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Book One"),
                &["Author"],
                &[],
                Some(("Poppy War", "1")),
                None,
            ),
            indexed(
                "b.epub",
                Some("Book Two"),
                &["Author"],
                &[],
                Some(("Poppy War", "2")),
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "poppy").await.unwrap();
    assert!(!results.series.is_empty(), "should match series substring");
    assert_eq!(results.series[0].name, "Poppy War");
    assert_eq!(results.series[0].book_count, 2);
    // F1 results page: the "incl. <title>" line draws on the first book in
    // the series by sort order.
    assert_eq!(
        results.series[0].lead_book_title.as_deref(),
        Some("Book One"),
        "series hit should carry its lead book title"
    );
}

#[tokio::test]
async fn search_palette_tags_match() {
    let _covers = CoversTempDir::new("palette_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Author"],
            &["Dark academia"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "academia").await.unwrap();
    assert!(!results.tags.is_empty(), "should match tag substring");
    assert_eq!(results.tags[0].name, "Dark academia");
    assert_eq!(results.tags[0].book_count, 1);
}

// #128: lock the wiring between the palette and `build_fts_match`'s
// facet prefixes. A regression in the facet parser could otherwise
// silently break palette tag:/author:/series: queries without any
// palette test failing.
#[tokio::test]
async fn search_palette_book_matches_tag_facet() {
    let _covers = CoversTempDir::new("palette_tag_facet");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &["vampires"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("Frankenstein"),
                &["Mary Shelley"],
                &["monsters"],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "tag:vampires").await.unwrap();
    let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
    assert!(
        titles.contains(&"Dracula"),
        "tag:vampires should match Dracula, got {titles:?}"
    );
    assert!(
        !titles.contains(&"Frankenstein"),
        "tag:vampires should not match Frankenstein"
    );
}

#[tokio::test]
async fn search_palette_book_matches_author_facet() {
    let _covers = CoversTempDir::new("palette_author_facet");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &["horror"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("Frankenstein"),
                &["Mary Shelley"],
                &["horror"],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "author:stoker")
        .await
        .unwrap();
    let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
    assert!(
        titles.contains(&"Dracula"),
        "author:stoker should match Dracula, got {titles:?}"
    );
    assert!(
        !titles.contains(&"Frankenstein"),
        "author:stoker should not match Frankenstein"
    );
}

#[tokio::test]
async fn search_palette_book_matches_series_facet() {
    let _covers = CoversTempDir::new("palette_series_facet");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Book One"),
                &["Author A"],
                &[],
                Some(("Dracula Chronicles", "1")),
                None,
            ),
            indexed("b.epub", Some("Unrelated"), &["Author B"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "series:dracula")
        .await
        .unwrap();
    let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
    assert!(
        titles.contains(&"Book One"),
        "series:dracula should match Book One, got {titles:?}"
    );
    assert!(
        !titles.contains(&"Unrelated"),
        "series:dracula should not match Unrelated"
    );
}

#[tokio::test]
async fn search_palette_empty_query_returns_empty() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let results = search_palette(&pool, "/lib", "   ").await.unwrap();
    assert!(results.books.is_empty());
    assert!(results.authors.is_empty());
    assert!(results.series.is_empty());
    assert!(results.tags.is_empty());
    assert_eq!(results.query, "");
}

#[tokio::test]
async fn search_palette_no_results() {
    let _covers = CoversTempDir::new("palette_no_results");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "zzzznonexistent")
        .await
        .unwrap();
    assert!(results.books.is_empty());
    assert!(results.authors.is_empty());
    assert!(results.series.is_empty());
    assert!(results.tags.is_empty());
}

#[tokio::test]
async fn search_palette_scoped_to_library() {
    let _covers = CoversTempDir::new("palette_scoped");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib-a",
        vec![indexed(
            "a.epub",
            Some("Alpha"),
            &["Tolkien"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib-b",
        vec![indexed(
            "b.epub",
            Some("Beta"),
            &["Tolkien"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib-a", "tolkien").await.unwrap();
    // Books should only include lib-a
    assert_eq!(results.books.len(), 1);
    assert_eq!(results.books[0].title, "Alpha");
    // Author book_count should be 1 (scoped to lib-a), not 2
    assert_eq!(results.authors[0].book_count, 1);
}

#[tokio::test]
async fn search_palette_duration_populated() {
    let _covers = CoversTempDir::new("palette_duration");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
    )
    .await
    .unwrap();

    let results = search_palette(&pool, "/lib", "author").await.unwrap();
    // duration_ms should be populated (at least 0 — we just check it's set)
    assert!(results.duration_ms < 10000, "duration should be reasonable");
}

#[tokio::test]
async fn search_palette_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = search_palette(&pool, "/lib", "query").await.unwrap_err();
    assert!(matches!(err, PaletteError::Db(_)));
}

#[test]
fn palette_error_from_metadata_overrides_error_returns_other_for_bulk_write_variants() {
    let err = PaletteError::from(
        crate::metadata_overrides::MetadataOverridesError::BookNotFound("abc".into()),
    );
    assert!(
        matches!(&err, PaletteError::Other(msg) if msg.contains("abc")),
        "expected Other carrying the source message, got {err:?}"
    );
}
