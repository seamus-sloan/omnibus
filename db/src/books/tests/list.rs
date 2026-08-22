//! `list_books` and the `library_from_db*` wrappers: the server-side response
//! cap, per-library and combined listings, counts, author filtering, and the
//! multi-valued / multi-format row shape.

use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, seed_minimal_books, series_id_by_name, CoversTempDir};

use super::super::*;

// ---------- Server-side cap (issue #81) ----------
//
// `list_books` / `search_books` previously had no `LIMIT`, so a single
// `/api/ebooks` poll on a multi-thousand-book library serialized the
// whole table. The fix is a hard `LIMIT MAX_BOOKS_RETURNED`, plus a
// companion count helper so callers can detect truncation.

#[tokio::test]
async fn library_from_db_returns_empty_for_none_path() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = library_from_db(&pool, None).await.unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
    assert!(lib.error.is_none());
}

#[tokio::test]
async fn list_books_caps_response_at_max_books_returned() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_BOOKS_RETURNED + 25;
    seed_minimal_books(&pool, total).await;

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        books.len() as i64,
        MAX_BOOKS_RETURNED,
        "list_books must cap the returned vec at MAX_BOOKS_RETURNED"
    );

    let counted = count_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        counted, total,
        "count_books must report the true row count (uncapped)"
    );
}

#[tokio::test]
async fn count_books_returns_zero_for_unknown_library() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(count_books(&pool, "/nope").await.unwrap(), 0);
}

#[tokio::test]
async fn library_from_db_with_total_reports_truncation() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_BOOKS_RETURNED + 7;
    seed_minimal_books(&pool, total).await;

    let (lib, returned_total) = library_from_db_with_total(&pool, Some("/lib"))
        .await
        .unwrap();
    assert_eq!(lib.path.as_deref(), Some("/lib"));
    assert_eq!(lib.books.len() as i64, MAX_BOOKS_RETURNED);
    assert_eq!(returned_total, total);
    assert!(
        returned_total > MAX_BOOKS_RETURNED,
        "test must seed strictly more rows than the cap"
    );
}

#[tokio::test]
async fn library_from_db_with_total_reports_zero_for_none_path() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (lib, total) = library_from_db_with_total(&pool, None).await.unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
    assert_eq!(total, 0);
}

#[tokio::test]
async fn library_from_db_combined_returns_books_from_both_paths() {
    let _covers = CoversTempDir::new("combined_landing");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/ebooks",
        vec![indexed("a.epub", Some("Ebook A"), &[], &[], None, None)],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/audiobooks",
        vec![
            indexed("b.m4b", Some("Audio B"), &[], &[], None, None),
            indexed("c.m4b", Some("Audio C"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let lib = library_from_db_combined(&pool, Some("/ebooks"), Some("/audiobooks"))
        .await
        .unwrap();
    let titles: Vec<_> = lib.books.iter().filter_map(|b| b.title.clone()).collect();
    assert!(titles.contains(&"Ebook A".to_string()));
    assert!(titles.contains(&"Audio B".to_string()));
    assert!(titles.contains(&"Audio C".to_string()));
    assert_eq!(lib.books.len(), 3);
    assert_eq!(lib.path.as_deref(), Some("/ebooks"));
}

#[tokio::test]
async fn library_from_db_combined_falls_back_to_audiobook_path_for_subtitle() {
    let _covers = CoversTempDir::new("combined_audio_only");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/audiobooks",
        vec![indexed("a.m4b", Some("Audio Only"), &[], &[], None, None)],
    )
    .await
    .unwrap();

    let lib = library_from_db_combined(&pool, None, Some("/audiobooks"))
        .await
        .unwrap();
    assert_eq!(lib.books.len(), 1);
    assert_eq!(lib.path.as_deref(), Some("/audiobooks"));
}

#[tokio::test]
async fn library_from_db_combined_returns_empty_when_both_paths_none() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = library_from_db_combined(&pool, None, None).await.unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
}

#[tokio::test]
async fn library_from_db_with_total_combined_counts_books_across_paths() {
    let _covers = CoversTempDir::new("combined_total");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/ebooks",
        vec![indexed("a.epub", Some("E1"), &[], &[], None, None)],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/audiobooks",
        vec![
            indexed("b.m4b", Some("A1"), &[], &[], None, None),
            indexed("c.m4b", Some("A2"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let (lib, total) =
        library_from_db_with_total_combined(&pool, Some("/ebooks"), Some("/audiobooks"))
            .await
            .unwrap();
    assert_eq!(lib.books.len(), 3);
    assert_eq!(total, 3);
}

#[tokio::test]
async fn library_from_db_combined_dedupes_shared_path() {
    let _covers = CoversTempDir::new("combined_shared");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/shared",
        vec![
            indexed("a.epub", Some("Ebook"), &[], &[], None, None),
            indexed("b.m4b", Some("Audio"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();

    // Both paths point at the same on-disk root — `IN (?, ?)` with a
    // duplicate would still return one row per book, but the helper
    // dedupes so the input shape stays consistent with single-library
    // callsites that rely on `library_paths.len()`.
    let lib = library_from_db_combined(&pool, Some("/shared"), Some("/shared"))
        .await
        .unwrap();
    assert_eq!(lib.books.len(), 2);
}

#[tokio::test]
async fn list_books_populates_formats_from_book_files() {
    // Regression: F1.7 power-user table & inline format chips read
    // `EbookMetadata.formats` off the list endpoint; if list_books
    // returned `vec![]` the chip row would hide itself entirely.
    let _covers = CoversTempDir::new("list_books_formats");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alpha.epub",
            Some("Alpha"),
            &["Author A"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].formats, vec!["EPUB".to_string()]);
}

#[tokio::test]
async fn list_books_returns_one_row_per_book_with_multi_format() {
    // Regression for PR #74 review: adding a second physical file
    // (EPUB + M4B) used to duplicate the parent row because the outer
    // query LEFT-JOINed `book_files`. The chip facets / table would
    // then over-count. Both queries now use scalar subqueries so the
    // result is one row per `books.id` and `.formats` carries both.
    let _covers = CoversTempDir::new("list_books_multi");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alpha.epub",
            Some("Alpha"),
            &["Author A"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let id = list_books(&pool, "/lib").await.unwrap()[0].id;
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes)
         VALUES (?, 'M4B', 'alpha', 0)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1, "multi-format must not duplicate rows");
    assert_eq!(
        books[0].formats,
        vec!["EPUB".to_string(), "M4B".to_string()]
    );
    // EPUB wins the primary-filename tiebreak, matching get_book.
    assert_eq!(books[0].filename, "alpha.epub");
}

#[tokio::test]
async fn list_books_filters_by_author_join() {
    let _covers = CoversTempDir::new("filter_author");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
            indexed("b.epub", Some("B"), &["Pratchett"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let titles: Vec<String> = sqlx::query_scalar(
        "SELECT b.title FROM books b
         JOIN books_authors_link bal ON bal.book = b.id
         JOIN authors a ON a.id = bal.author
         WHERE a.name = ?",
    )
    .bind("Tolkien")
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(titles, vec!["A".to_string()]);
}

/// Regression guard for the N+1 fix in list_books / search_books.
/// Both functions must return all creators, subjects, and identifiers
/// via the inline json_group_array subqueries rather than per-book
/// round-trip calls.
#[tokio::test]
async fn list_and_search_books_return_multi_valued_fields() {
    let _covers = CoversTempDir::new("multi_valued");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "multi.epub".into(),
                title: Some("Multi".into()),
                creators: vec![
                    Contributor {
                        name: "Alice".into(),
                        ..Default::default()
                    },
                    Contributor {
                        name: "Bob".into(),
                        ..Default::default()
                    },
                ],
                subjects: vec!["Fiction".into(), "Sci-Fi".into()],
                identifiers: vec![
                    Identifier {
                        value: "978-0-000000-00-0".into(),
                        scheme: Some("isbn".into()),
                    },
                    Identifier {
                        value: "https://example.com/book/1".into(),
                        scheme: Some("uri".into()),
                    },
                ],
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let book = &books[0];
    assert_eq!(
        book.creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Alice", "Bob"]
    );
    assert_eq!(
        book.subjects,
        vec!["Fiction".to_string(), "Sci-Fi".to_string()]
    );
    assert_eq!(book.identifiers.len(), 2);
    assert_eq!(book.identifiers[0].scheme.as_deref(), Some("isbn"));
    assert_eq!(book.identifiers[1].scheme.as_deref(), Some("uri"));

    let hits = search_books(&pool, "/lib", "Multi").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        hits[0]
            .creators
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        vec!["Alice", "Bob"]
    );
    assert_eq!(
        hits[0].subjects,
        vec!["Fiction".to_string(), "Sci-Fi".to_string()]
    );
    assert_eq!(hits[0].identifiers.len(), 2);
}

// Additional coverage for core book query functions.
#[tokio::test]
async fn list_books_filters_by_library_path() {
    let _covers = CoversTempDir::new("list_books_filter_lib");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib-a",
        vec![
            indexed("a1.epub", Some("Alpha One"), &["Author A"], &[], None, None),
            indexed("a2.epub", Some("Alpha Two"), &["Author A"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib-b",
        vec![indexed(
            "b1.epub",
            Some("Beta One"),
            &["Author B"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let lib_a = list_books(&pool, "/lib-a").await.unwrap();
    let lib_b = list_books(&pool, "/lib-b").await.unwrap();

    assert_eq!(lib_a.len(), 2, "lib-a should return only its two books");
    let mut titles_a: Vec<String> = lib_a.iter().filter_map(|b| b.title.clone()).collect();
    titles_a.sort();
    assert_eq!(titles_a, vec!["Alpha One", "Alpha Two"]);

    assert_eq!(lib_b.len(), 1, "lib-b should return only its one book");
    assert_eq!(lib_b[0].title.as_deref(), Some("Beta One"));
}

#[tokio::test]
async fn list_books_returns_empty_for_unknown_path() {
    let _covers = CoversTempDir::new("list_books_unknown");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Title"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let hits = list_books(&pool, "/does-not-exist").await.unwrap();
    assert!(
        hits.is_empty(),
        "unknown library path should yield an empty vec (no error)"
    );
}

#[tokio::test]
async fn list_books_returns_empty_for_empty_db() {
    let _covers = CoversTempDir::new("list_books_empty_db");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let hits = list_books(&pool, "/lib").await.unwrap();
    assert!(hits.is_empty(), "empty DB should yield an empty vec");
}

// ---------- F5: collapsed json_object projection (primary file + series) ----------

#[tokio::test]
async fn list_books_returns_collapsed_primary_file_and_series_for_book_with_series_and_multiple_formats(
) {
    // F5: `primary_filename`/`primary_format` and `series_name`/`series_link_id`
    // each used to be two correlated subqueries scanning the same row twice;
    // they're now single `json_object` subqueries. This is the equality oracle:
    // a book that HAS a series AND multiple formats must still yield the
    // EPUB-primary filename/format and the right series name + id.
    let _covers = CoversTempDir::new("collapse_primary_and_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "saga-book.epub",
            Some("Saga Book"),
            &["Author A"],
            &[],
            Some(("Saga", "3")),
            None,
        )],
    )
    .await
    .unwrap();
    let id = list_books(&pool, "/lib").await.unwrap()[0].id;
    // Add a second physical format so the EPUB-preferred tiebreak is exercised.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes)
         VALUES (?, 'M4B', 'saga-book', 0)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1, "multi-format must not duplicate rows");
    let book = &books[0];
    // Collapsed primary_file_json: EPUB wins the tiebreak, format lowercased.
    assert_eq!(book.filename, "saga-book.epub");
    assert_eq!(book.formats, vec!["EPUB".to_string(), "M4B".to_string()]);
    // Collapsed series_json: name + id come from the same picked row.
    assert_eq!(book.series.as_deref(), Some("Saga"));
    assert_eq!(book.series_index.as_deref(), Some("3"));
    let expected_series_id = series_id_by_name(&pool, "Saga").await;
    assert_eq!(book.series_id, Some(expected_series_id));
}

#[tokio::test]
async fn list_books_returns_no_series_when_book_has_none_after_collapse() {
    // The collapsed `series_json` subquery returns SQLite NULL (decoded to
    // `None`) for a book with no `books_series_link` row — same shape the
    // former two-NULL-column pair produced.
    let _covers = CoversTempDir::new("collapse_no_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "standalone.epub",
            Some("Standalone"),
            &["Author C"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(books[0].filename, "standalone.epub");
    assert_eq!(books[0].series, None);
    assert_eq!(books[0].series_id, None);
}
