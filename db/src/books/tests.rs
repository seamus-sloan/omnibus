//! Inline unit tests for the `books` module. Kept in a single file so the
//! many cross-cutting helpers (`seed_minimal_books`, etc.) and the tests that
//! drive `list_books` + `search_books` + `get_book` together stay co-located.

use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};

use super::*;
use crate::ebook::IndexedBook;
use crate::helpers::MAX_QUERY_LEN;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{
    author_id_by_name, indexed, seed_discovery_fixture, seed_minimal_books, series_id_by_name,
    CoversTempDir,
};

// ---------- Server-side cap (issue #81) ----------
//
// `list_books` / `search_books` previously had no `LIMIT`, so a single
// `/api/ebooks` poll on a multi-thousand-book library serialized the
// whole table. The fix is a hard `LIMIT MAX_BOOKS_RETURNED`, plus a
// companion count helper so callers can detect truncation.

#[tokio::test]
async fn get_book_formats_machine_timestamps_as_fixed_width_iso() {
    // Migration 0038 stores `timestamp`/`last_modified` as INTEGER epochs; the
    // projection formats them back to fixed-width ISO so the wire
    // `added_at`/`modified` stay `Option<String>` and sort lexicographically.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, timestamp, last_modified) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book', \
                 strftime('%s','2024-01-02 03:04:05'), strftime('%s','2020-06-15 12:00:00')) \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 1, 1)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.added_at.as_deref(), Some("2024-01-02T03:04:05Z"));
    assert_eq!(book.modified.as_deref(), Some("2020-06-15T12:00:00Z"));
}

#[tokio::test]
async fn book_last_modified_for_returns_the_stored_epoch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book', 1700000000) \
         RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    let last_modified = book_last_modified_for(&pool, id).await.unwrap();
    assert_eq!(last_modified, 1700000000);
}

#[tokio::test]
async fn book_last_modified_for_defaults_to_zero_when_column_is_null() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let last_modified = book_last_modified_for(&pool, id).await.unwrap();
    assert_eq!(last_modified, 0);
}

#[tokio::test]
async fn book_last_modified_for_returns_db_error_for_unknown_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let err = book_last_modified_for(&pool, 999).await.unwrap_err();
    assert!(matches!(err, BooksError::Db(_)));
}

#[tokio::test]
async fn get_book_reports_epub_size_from_lowest_ordinal_epub() {
    // `epub_size_bytes` drives the export menu's Kindle size gate. It must
    // mirror what the hero send delivers — the lowest-ordinal EPUB — and ignore
    // non-EPUB files entirely.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Two EPUB editions (ordinal 0 wins) plus an audiobook that must be ignored.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal) VALUES \
         (?1, 'EPUB', 'a', 111, 0), (?1, 'EPUB', 'b', 222, 1), (?1, 'M4B', 'c', 999, 0)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.epub_size_bytes, Some(111));
}

#[tokio::test]
async fn get_book_reports_no_epub_size_for_audio_only_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal) \
         VALUES (?, 'M4B', 'c', 999, 0)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.epub_size_bytes, None);
}

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
async fn search_books_finds_by_title_and_ranks_by_bm25() {
    let _covers = CoversTempDir::new("fts_title");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Harry Potter"),
                &["J.K. Rowling"],
                &[],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("Something Else"),
                &["Author B"],
                &["harry"],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "harry").await.unwrap();
    // Column filter scopes MATCH to title/authors/series — the tag-only
    // hit on "Something Else" is intentionally excluded.
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("Harry Potter"));
}
#[tokio::test]
async fn search_books_with_total_matches_count_search_books() {
    // #241: the single FTS5 pass must return the same true hit count the
    // standalone `count_search_books` query produced.
    let _covers = CoversTempDir::new("fts_total");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Rust in Action"), &["A"], &[], None, None),
            indexed(
                "b.epub",
                Some("Rust for Rustaceans"),
                &["B"],
                &[],
                None,
                None,
            ),
            indexed("c.epub", Some("Unrelated"), &["C"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let (books, total) = search_books_with_total(&pool, "/lib", "rust")
        .await
        .unwrap();
    assert_eq!(books.len(), 2, "two titles match 'rust'");
    assert_eq!(
        total, 2,
        "single-pass total (scalar COUNT over the materialized CTE) equals the match count"
    );
    let counted = count_search_books(&pool, "/lib", "rust").await.unwrap();
    assert_eq!(
        total, counted,
        "single-pass total agrees with count_search_books"
    );

    // Empty query short-circuits to (empty, 0) without an FTS pass.
    let (empty, zero) = search_books_with_total(&pool, "/lib", "   ").await.unwrap();
    assert!(empty.is_empty());
    assert_eq!(zero, 0);
}
#[tokio::test]
async fn search_books_finds_by_author_and_scopes_to_library() {
    let _covers = CoversTempDir::new("fts_author");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib-a",
        vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib-b",
        vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib-a", "tolkien").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("A"));
}
#[tokio::test]
async fn search_books_truncates_oversized_query() {
    // Issue #189: a query longer than MAX_QUERY_LEN chars must be capped
    // before reaching build_fts_match, not panic or pass an unbounded
    // expression to FTS5. The exact rows don't matter — this documents
    // the contract that oversized input is bounded and returns Ok.
    let _covers = CoversTempDir::new("fts_oversized");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Harry Potter"),
            &["J.K. Rowling"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    // A single token far longer than the cap (no whitespace) is the
    // worst case the issue describes.
    let oversized = "a".repeat(MAX_QUERY_LEN * 10);
    assert!(oversized.chars().count() > MAX_QUERY_LEN);

    let hits = search_books(&pool, "/lib", &oversized).await;
    assert!(hits.is_ok(), "oversized query should not error");

    let total = count_search_books(&pool, "/lib", &oversized).await;
    assert!(total.is_ok(), "oversized count query should not error");
}
#[tokio::test]
async fn search_books_empty_query_returns_empty_vec() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let hits = search_books(&pool, "/lib", "   ").await.unwrap();
    assert!(hits.is_empty());
}
#[tokio::test]
async fn search_books_handles_unbalanced_quote_without_error() {
    let _covers = CoversTempDir::new("fts_unbalanced");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Quoted"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    // Unbalanced `"` in raw input must not surface as a MATCH parse error.
    let hits = search_books(&pool, "/lib", "say \"hi")
        .await
        .expect("sanitizer guards against MATCH parse errors");
    assert!(hits.is_empty());
}
#[tokio::test]
async fn search_books_excludes_isbn_column_from_match() {
    // ISBN is indexed in books_fts but the search column filter scopes
    // MATCH to title/authors/series, so ISBN lookups are intentionally
    // not surfaced. When/if we re-enable ISBN search, flip this to
    // assert a hit — no migration required.
    let _covers = CoversTempDir::new("fts_isbn");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut meta = indexed("a.epub", Some("ISBN Book"), &["A"], &[], None, None).metadata;
    meta.identifiers.push(Identifier {
        value: "978-0-123456-78-9".into(),
        scheme: Some("isbn".into()),
    });
    replace_books(
        &pool,
        "/lib",
        vec![IndexedBook {
            metadata: meta,
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "978-0-123456-78-9")
        .await
        .unwrap();
    assert!(hits.is_empty());
}
#[tokio::test]
async fn search_books_author_facet_filters_to_matching_author() {
    let _covers = CoversTempDir::new("fts_facet_author");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Pride and Prejudice"),
                &["Austen"],
                &[],
                None,
                None,
            ),
            indexed("b.epub", Some("Hamlet"), &["Shakespeare"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "author:austen").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
}
#[tokio::test]
async fn search_books_series_facet_filters_to_matching_series() {
    let _covers = CoversTempDir::new("fts_facet_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Dune"),
                &["Herbert"],
                &[],
                Some(("Dune Saga", "1")),
                None,
            ),
            indexed("b.epub", Some("Standalone"), &["Author"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "series:dune").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("Dune"));
}
#[tokio::test]
async fn search_books_tag_facet_filters_to_matching_tag() {
    let _covers = CoversTempDir::new("fts_facet_tag");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
            indexed("b.epub", Some("B"), &["Y"], &["history"], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "tag:fiction").await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("A"));
}
#[tokio::test]
async fn search_books_facet_combines_with_free_text_via_explicit_and() {
    let _covers = CoversTempDir::new("fts_facet_combined");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Pride and Prejudice"),
                &["Austen"],
                &[],
                None,
                None,
            ),
            indexed("b.epub", Some("Emma"), &["Austen"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    // Both clauses must match — only Pride and Prejudice carries the
    // "pride" token in title/authors/series.
    let hits = search_books(&pool, "/lib", "author:austen pride")
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
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
async fn search_books_returns_one_row_per_book_with_multi_format() {
    // Same regression in the FTS path.
    let _covers = CoversTempDir::new("search_books_multi");
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

    let results = search_books(&pool, "/lib", "Alpha").await.unwrap();
    assert_eq!(results.len(), 1, "FTS results must not duplicate rows");
    assert_eq!(
        results[0].formats,
        vec!["EPUB".to_string(), "M4B".to_string()]
    );
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

/// F8 regression: the denormalized `books.isbn` column was dropped (migration
/// 0023). A book's ISBN must still surface through the identifier projection,
/// which reads the canonical `book_identifiers` rows — proving the read path
/// never depended on the removed column.
#[tokio::test]
async fn get_book_and_list_books_surface_isbn_from_book_identifiers_after_column_drop() {
    let _covers = CoversTempDir::new("isbn_after_drop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "isbn.epub".into(),
                title: Some("ISBN Book".into()),
                identifiers: vec![Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("isbn".into()),
                }],
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

    // The `books` table no longer has an `isbn` column at all.
    let has_isbn_col: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM pragma_table_info('books') WHERE name = 'isbn'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(has_isbn_col, 0, "books.isbn must be dropped");

    let list = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(list.len(), 1);
    let list_isbn = list[0]
        .identifiers
        .iter()
        .find(|i| i.scheme.as_deref() == Some("isbn"))
        .map(|i| i.value.as_str());
    assert_eq!(list_isbn, Some("9780000000000"));

    let detail = get_book(&pool, list[0].id).await.unwrap().unwrap();
    let detail_isbn = detail
        .identifiers
        .iter()
        .find(|i| i.scheme.as_deref() == Some("isbn"))
        .map(|i| i.value.as_str());
    assert_eq!(detail_isbn, Some("9780000000000"));
}

#[tokio::test]
async fn get_book_returns_none_for_missing_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let result = get_book(&pool, 9999).await.unwrap();
    assert!(result.is_none());
}
#[tokio::test]
async fn get_book_is_deterministic_with_multiple_files_and_links() {
    // Regression for PR #55 review: when a book has multiple `book_files`
    // rows (and incidental duplicate publisher/language/series links),
    // get_book() must return the EPUB-preferred filename and stable
    // publisher/language/series values rather than whichever joined row
    // SQLite happens to return first.
    let _covers = CoversTempDir::new("get_book_multi");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alpha.epub",
            Some("Alpha Book"),
            &["Author A"],
            &["Fiction"],
            Some(("Saga", "1")),
            None,
        )],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let id = books[0].id;

    // Add a second physical file in another format.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes)
         VALUES (?, 'M4B', 'alpha', 0)",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    // Add a second publisher and a second language to exercise the
    // multi-row JOIN path on those link tables. Series already has one
    // row from `replace_books`.
    sqlx::query("INSERT INTO publishers (name) VALUES ('Acme'), ('Zenith')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books_publishers_link (book, publisher)
         SELECT ?, id FROM publishers WHERE name IN ('Acme', 'Zenith')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO languages (code) VALUES ('eng'), ('fra')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books_languages_link (book, language)
         SELECT ?, id FROM languages WHERE code IN ('eng', 'fra')",
    )
    .bind(id)
    .execute(&pool)
    .await
    .unwrap();

    // Run get_book repeatedly — every call must return identical values.
    let first = get_book(&pool, id).await.unwrap().expect("book");
    for _ in 0..3 {
        let again = get_book(&pool, id).await.unwrap().expect("book");
        assert_eq!(again.filename, first.filename);
        assert_eq!(again.publisher, first.publisher);
        assert_eq!(again.language, first.language);
        assert_eq!(again.series, first.series);
        assert_eq!(again.formats, first.formats);
    }

    // EPUB should win the tiebreak for filename.
    assert_eq!(first.filename, "alpha.epub");
    // Both formats surface in the formats list, sorted by format code.
    assert_eq!(first.formats, vec!["EPUB".to_string(), "M4B".to_string()]);
    // Publisher/language pick alphabetical winners deterministically.
    assert_eq!(first.publisher.as_deref(), Some("Acme"));
    assert_eq!(first.language.as_deref(), Some("eng"));
    assert_eq!(first.series.as_deref(), Some("Saga"));
    assert_eq!(first.series_index.as_deref(), Some("1"));
}
#[tokio::test]
async fn get_book_returns_metadata_for_indexed_book() {
    let _covers = CoversTempDir::new("get_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alpha.epub",
            Some("Alpha Book"),
            &["Author A"],
            &["Fiction"],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let id = books[0].id;

    let book = get_book(&pool, id)
        .await
        .unwrap()
        .expect("book should exist");
    assert_eq!(book.id, id);
    assert_eq!(book.title.as_deref(), Some("Alpha Book"));
    assert_eq!(book.creators.len(), 1);
    assert_eq!(book.creators[0].name, "Author A");
    assert_eq!(book.subjects, vec!["Fiction"]);
    assert!(!book.formats.is_empty(), "formats should be populated");
    assert!(
        book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub")),
        "EPUB format should be present"
    );
}
#[tokio::test]
async fn get_book_handles_book_with_no_relations() {
    // A `books` row that has zero m2m link rows and zero files: every
    // `json_group_array` subquery returns "[]" (over zero inner rows) and
    // every scalar subquery returns NULL. The function must still return
    // a populated `EbookMetadata` with empty vecs and an empty filename
    // rather than erroring out on the missing data.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_res = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let lib_id = lib_res.last_insert_rowid();
    let res = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('lonely-uuid', ?, '/lib/lonely', 'Lonely')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    let id = res.last_insert_rowid();

    let book = get_book(&pool, id)
        .await
        .unwrap()
        .expect("book should exist");
    assert_eq!(book.id, id);
    assert_eq!(book.title.as_deref(), Some("Lonely"));
    assert_eq!(book.filename, "");
    assert!(book.creators.is_empty());
    assert!(book.subjects.is_empty());
    assert!(book.identifiers.is_empty());
    assert!(book.formats.is_empty());
    assert!(book.publisher.is_none());
    assert!(book.language.is_none());
    assert!(book.series.is_none());
    assert!(book.cover_url.is_none());
}
#[tokio::test]
async fn get_book_round_trips_values_containing_control_chars_and_quotes() {
    // Regression for PR #65 review: prior delimiter-based encoding
    // (`GROUP_CONCAT` with 0x1F/0x1E separators) would have silently
    // corrupted any value containing those control chars. The JSON
    // encoding must survive arbitrary UTF-8 — control chars, quotes,
    // backslashes, commas — without altering the round-tripped value.
    let _covers = CoversTempDir::new("get_book_collide");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let nasty_name = "Smith\u{001F}, John \"O'Reilly\" \\back\u{001E}/";
    let nasty_tag = "Sci-Fi\u{001F}Drama";
    let nasty_value = "9780\u{001E}123\"456\\";
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alpha.epub",
            Some("Alpha"),
            &[nasty_name],
            &[nasty_tag],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let id = list_books(&pool, "/lib").await.unwrap()[0].id;
    sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?, 'ISBN', ?)")
        .bind(id)
        .bind(nasty_value)
        .execute(&pool)
        .await
        .unwrap();

    let book = get_book(&pool, id).await.unwrap().expect("book");
    assert_eq!(book.creators.len(), 1);
    assert_eq!(book.creators[0].name, nasty_name);
    assert_eq!(book.subjects, vec![nasty_tag.to_string()]);
    assert_eq!(book.identifiers.len(), 1);
    assert_eq!(book.identifiers[0].value, nasty_value);
    assert_eq!(book.identifiers[0].scheme.as_deref(), Some("ISBN"));
}
#[test]
fn sanitize_description_preserves_safe_html() {
    let cleaned = sanitize_description(Some(
        "<p>Hello <strong>world</strong>!</p><p>Second <em>line</em>.</p>".into(),
    ))
    .unwrap();
    assert!(cleaned.contains("<p>"));
    assert!(cleaned.contains("<strong>world</strong>"));
    assert!(cleaned.contains("<em>line</em>"));
}
#[test]
fn sanitize_description_strips_scripts_and_event_handlers() {
    // ammonia's defaults must drop <script>, inline `onerror`, and
    // `javascript:` URLs. Anything that could execute on the detail page
    // when rendered via dangerous_inner_html is the threat model here.
    let cleaned = sanitize_description(Some(
        "<p>Safe</p><script>alert('xss')</script>\
         <img src=x onerror=\"alert(1)\"/>\
         <a href=\"javascript:alert(1)\">click</a>"
            .into(),
    ))
    .unwrap();
    assert!(!cleaned.contains("<script"));
    assert!(!cleaned.to_ascii_lowercase().contains("onerror"));
    assert!(!cleaned.to_ascii_lowercase().contains("javascript:"));
    assert!(cleaned.contains("<p>Safe</p>"));
}
#[test]
fn sanitize_description_collapses_empty_input_to_none() {
    assert_eq!(sanitize_description(None), None);
    assert_eq!(sanitize_description(Some(String::new())), None);
    assert_eq!(sanitize_description(Some("   \n\t".into())), None);
    // A bare <script> with no other content sanitizes to "" and must
    // collapse to None so the UI hides the description block entirely.
    assert_eq!(
        sanitize_description(Some("<script>alert(1)</script>".into())),
        None
    );
}
#[tokio::test]
async fn get_book_returns_sanitized_html_description() {
    let _covers = CoversTempDir::new("sanitize_desc");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let raw = "<p>Brief.</p><script>alert('xss')</script><p>More <b>detail</b>.</p>".to_string();
    replace_books(
        &pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "alpha.epub".into(),
                title: Some("Alpha".into()),
                description: Some(raw),
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
    let id = list_books(&pool, "/lib").await.unwrap()[0].id;

    let desc = get_book(&pool, id).await.unwrap().unwrap().description;
    let desc = desc.expect("description should be present");
    assert!(desc.contains("<p>Brief.</p>"));
    assert!(desc.contains("<b>detail</b>"));
    assert!(!desc.contains("<script"));
}
#[tokio::test]
async fn get_book_merges_scalar_overrides() {
    let _covers = CoversTempDir::new("merge_scalar");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "merge.epub",
            Some("Original Title"),
            &["Author A"],
            &["fiction"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let book = &books[0];
    let uuid = book.unique_identifier.clone().unwrap();
    let id = book.id;

    // Save overrides.
    let ov = MetadataOverrides {
        title: Some("Edited Title".into()),
        publisher: Some("New Publisher".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    // get_book should return merged values.
    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.title.as_deref(), Some("Edited Title"));
    assert_eq!(merged.publisher.as_deref(), Some("New Publisher"));
    assert!(merged.has_override);
    // Non-overridden fields unchanged.
    assert_eq!(merged.creators[0].name, "Author A");
}
#[tokio::test]
async fn get_book_merges_creators_replaces_entirely() {
    let _covers = CoversTempDir::new("merge_creators");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "creators.epub",
            Some("Book"),
            &["Author A"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let id = books[0].id;

    let ov = MetadataOverrides {
        creators: Some(vec![
            Contributor {
                name: "Author B".into(),
                ..Default::default()
            },
            Contributor {
                name: "Author C".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 2);
    assert_eq!(merged.creators[0].name, "Author B");
    assert_eq!(merged.creators[1].name, "Author C");
}
#[tokio::test]
async fn get_book_merges_subjects_replaces_entirely() {
    let _covers = CoversTempDir::new("merge_subjects");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "subjects.epub",
            Some("Book"),
            &["Author"],
            &["fiction", "classic"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let id = books[0].id;

    let ov = MetadataOverrides {
        subjects: Some(vec!["sci-fi".into(), "adventure".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.subjects, vec!["sci-fi", "adventure"]);
}
#[tokio::test]
async fn get_book_backfills_creator_ids_after_override_replaces_authors() {
    // Override Contributors carry only a name, so a book whose author
    // list was edited through the metadata form would otherwise come
    // back with `creators[*].id == None`, rendering the breadcrumb's
    // author link as an unclickable span even when the `authors` row
    // exists. Verify get_book backfills the id by name. Mirrors the
    // user's report against book 268 (multi-author book where the
    // user removed all but one canonical author).
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    // saga1.epub canonically has ["Ada Lovelace", "Grace Hopper"];
    // simulate the user dropping the second author through the edit
    // form. apply_overrides replaces creators wholesale, so the
    // override Contributor has id = None.
    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Ada Lovelace".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "Ada Lovelace");
    assert_eq!(
        merged.creators[0].id,
        Some(ada_id),
        "creator id must be backfilled so the breadcrumb renders as a Link",
    );
}
#[tokio::test]
async fn get_book_backfills_creator_ids_case_insensitively() {
    // `authors.name` is `UNIQUE COLLATE NOCASE`, so a SQL `IN (...)`
    // lookup matches case-insensitively — but the returned row carries
    // the DB casing while the override carries the user-supplied
    // casing. The HashMap must normalise both sides to lowercase so
    // an override like "ada lovelace" still resolves to the canonical
    // "Ada Lovelace" id.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "ADA LOVELACE".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "ADA LOVELACE");
    assert_eq!(
        merged.creators[0].id,
        Some(ada_id),
        "case-mismatched override should still resolve to the canonical author id",
    );
}
#[tokio::test]
async fn get_book_leaves_creator_id_none_when_override_author_unknown() {
    // If the override sets an author name that doesn't exist in the
    // `authors` table, backfill must leave the id None.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Nobody Indexed".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "Nobody Indexed");
    assert_eq!(merged.creators[0].id, None);
}
#[tokio::test]
async fn get_book_backfills_series_id_from_override_when_series_exists() {
    // A book whose series was set via overrides (not at scan time)
    // historically came back with series_id == None even though the
    // series row existed in the relational table. The detail page's
    // "Series" rail then fell back to plain text instead of a Link
    // to /series/:id. Verify the read path now backfills the id.
    let _covers = CoversTempDir::new("override_series_link");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Seed: one book belongs to "Saga" natively (so the series row exists),
    // one standalone book that we'll later override into the same series.
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "saga1.epub",
                Some("Saga: Book One"),
                &["Author X"],
                &[],
                Some(("Saga", "1")),
                None,
            ),
            indexed("loner.epub", Some("Loner"), &["Author Y"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let saga_id = series_id_by_name(&pool, "Saga").await;
    let books = list_books(&pool, "/lib").await.unwrap();
    let loner = books.iter().find(|b| b.filename == "loner.epub").unwrap();
    assert_eq!(loner.series, None);
    assert_eq!(loner.series_id, None);
    let loner_uuid = loner.unique_identifier.clone().unwrap();
    let loner_book_id = loner.id;

    // Override the standalone to be part of "Saga". The overrides path
    // does not touch books_series_link, so loner.series_id stays unset
    // in the relational table — get_book must backfill from the series
    // table by name.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("3".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &loner_uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, loner_book_id).await.unwrap().unwrap();
    assert_eq!(merged.series.as_deref(), Some("Saga"));
    assert_eq!(
        merged.series_id,
        Some(saga_id),
        "override-only series must still resolve series_id so the detail rail can link"
    );
}
#[tokio::test]
async fn get_book_populates_series_id_when_override_creates_series() {
    // When an override sets a series name, `upsert_metadata_overrides`
    // materializes the `series` row + `books_series_link` so the
    // detail-page breadcrumb is clickable and `/series` lists it.
    let _covers = CoversTempDir::new("override_series_unknown");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alone.epub",
            Some("Alone"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let book = &books[0];
    let uuid = book.unique_identifier.clone().unwrap();
    let id = book.id;

    let ov = MetadataOverrides {
        series: Some("A Series That Does Not Yet Exist".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        merged.series.as_deref(),
        Some("A Series That Does Not Yet Exist")
    );
    assert!(
        merged.series_id.is_some(),
        "override should materialize series row so breadcrumb is clickable"
    );
}
#[tokio::test]
async fn list_books_merges_overrides_in_bulk() {
    let _covers = CoversTempDir::new("bulk_merge");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Book A"), &["Author A"], &[], None, None),
            indexed("b.epub", Some("Book B"), &["Author B"], &[], None, None),
            indexed("c.epub", Some("Book C"), &["Author C"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid_a = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book A"))
        .unwrap()
        .unique_identifier
        .clone()
        .unwrap();
    let uuid_c = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book C"))
        .unwrap()
        .unique_identifier
        .clone()
        .unwrap();

    // Override A and C only.
    upsert_metadata_overrides(
        &pool,
        &uuid_a,
        &MetadataOverrides {
            title: Some("Edited A".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    upsert_metadata_overrides(
        &pool,
        &uuid_c,
        &MetadataOverrides {
            title: Some("Edited C".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let a = books
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&uuid_a))
        .unwrap();
    let b = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book B"))
        .unwrap();
    let c = books
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&uuid_c))
        .unwrap();

    assert_eq!(a.title.as_deref(), Some("Edited A"));
    assert!(a.has_override);
    assert_eq!(b.title.as_deref(), Some("Book B"));
    assert!(!b.has_override);
    assert_eq!(c.title.as_deref(), Some("Edited C"));
    assert!(c.has_override);
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
#[tokio::test]
async fn search_books_handles_bare_asterisk_without_error() {
    // A raw `*` is an FTS5 operator; the sanitizer must quote it so MATCH
    // doesn't reject the expression as a syntax error. We assert the call
    // succeeds — not a particular hit shape — because the goal is "no panic
    // / no sqlx parse error".
    let _covers = CoversTempDir::new("fts_asterisk");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Anything"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "*")
        .await
        .expect("sanitizer should guard MATCH against the bare `*` operator");
    // `*` alone has no literal token to match; an empty result is fine.
    assert!(hits.is_empty());
}
#[tokio::test]
async fn search_books_handles_bare_double_quote_without_error() {
    // A raw `"` is the FTS5 phrase delimiter. Without sanitization, MATCH
    // would reject this with a parse error and the call would `Err`.
    let _covers = CoversTempDir::new("fts_dquote");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Anything"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib", "\"")
        .await
        .expect("sanitizer should guard MATCH against a bare `\"` operator");
    assert!(hits.is_empty());
}
#[tokio::test]
async fn search_books_returns_empty_for_unknown_library() {
    // Even with a real match in another library, the WHERE l.path = ?
    // clause must scope results to the requested library.
    let _covers = CoversTempDir::new("search_books_unknown_lib");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib-a",
        vec![indexed(
            "a.epub",
            Some("Findable"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let hits = search_books(&pool, "/lib-b", "Findable").await.unwrap();
    assert!(
        hits.is_empty(),
        "query against a non-existent library must not leak rows from another library"
    );
}

#[tokio::test]
async fn book_file_path_returns_absolute_path_for_epub() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    // `books.path` is stored RELATIVE to the library root (the scanner's
    // `root.join(filename)` convention), so the resolved path must be
    // `<libraries.path>/<books.path>/<stem>.<ext>`.
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-epub', ?, 'sub/dir', 'Some Book')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'some-book', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let path = book_file_path(&pool, book_id, "EPUB").await.unwrap();
    assert_eq!(
        path,
        Some(std::path::PathBuf::from("/lib/sub/dir/some-book.epub"))
    );
}

#[tokio::test]
async fn book_file_relative_dir_returns_library_relative_directory_for_epub() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-epub-rel', ?, 'sub/dir', 'Some Book')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'some-book', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let dir = book_file_relative_dir(&pool, book_id, "EPUB")
        .await
        .unwrap();
    // Relative to the scan root only — never includes `/lib`, unlike
    // `book_file_path`.
    assert_eq!(dir, Some(std::path::PathBuf::from("sub/dir")));
}

#[tokio::test]
async fn book_file_relative_dir_returns_none_for_missing_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = book_file_relative_dir(&pool, 9999, "EPUB").await.unwrap();
    assert!(dir.is_none());
}

#[tokio::test]
async fn book_file_path_returns_none_for_missing_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let path = book_file_path(&pool, 9999, "EPUB").await.unwrap();
    assert!(path.is_none());
}

#[tokio::test]
async fn book_file_path_returns_none_when_no_file_row_for_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-nofile', ?, '/lib/Bookless', 'Bookless')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    let path = book_file_path(&pool, book_id, "EPUB").await.unwrap();
    assert!(path.is_none());
}

#[tokio::test]
async fn book_file_paths_resolves_every_id_in_one_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_a = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-a', ?, 'sub/dir', 'Book A')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let book_b = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-b', ?, 'other', 'Book B')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'book-a', 0)",
    )
    .bind(book_a)
    .execute(&pool)
    .await
    .unwrap();
    // book_b has two EPUB files; the lower ordinal must win, same tie-break
    // as `book_file_path`.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes) \
         VALUES (?, 'EPUB', 'book-b-second', 1, 0)",
    )
    .bind(book_b)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes) \
         VALUES (?, 'EPUB', 'book-b-first', 0, 0)",
    )
    .bind(book_b)
    .execute(&pool)
    .await
    .unwrap();

    let map = book_file_paths(&pool, &[book_a, book_b, 9999], "EPUB")
        .await
        .unwrap();

    assert_eq!(map.len(), 2, "the unknown id must be absent, got {map:?}");
    assert_eq!(
        map.get(&book_a),
        Some(&std::path::PathBuf::from("/lib/sub/dir/book-a.epub"))
    );
    assert_eq!(
        map.get(&book_b),
        Some(&std::path::PathBuf::from("/lib/other/book-b-first.epub")),
        "the lower-ordinal file must win"
    );
}

#[tokio::test]
async fn book_file_paths_returns_empty_map_for_empty_ids() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let map = book_file_paths(&pool, &[], "EPUB").await.unwrap();
    assert!(map.is_empty());
}

#[tokio::test]
async fn book_file_paths_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = book_file_paths(&pool, &[1], "EPUB").await.unwrap_err();
    assert!(matches!(err, BooksError::Db(_)), "got {err:?}");
}

// ---------- list_indexed_rows_for_formats (#328) ----------

#[tokio::test]
async fn list_indexed_rows_for_formats_returns_only_matching_format_rows() {
    // Regression for #328: when ebook and audiobook libraries share a
    // path, the format-scoped read must return only the rows whose
    // `book_files.format` is in the allow-list.
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Seed one EPUB and one M4B row under the same library_path. Use
    // separate library rows to keep the seed helper simple — they share
    // the same `libraries.path` string only via the second seed adding
    // its own row, so we instead insert both books under the same id.
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/shared', '/shared') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let epub_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES ('uuid-epub', ?, '/shared/epub', 'EpubTitle', 'EpubTitle') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let m4b_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES ('uuid-m4b', ?, '/shared/audio', 'AudioTitle', 'AudioTitle') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'EpubTitle', 100, 100), \
                (?, 'M4B',  'AudioTitle', 200, 200)",
    )
    .bind(epub_id)
    .bind(m4b_id)
    .execute(&pool)
    .await
    .unwrap();

    let ebooks = list_indexed_rows_for_formats(&pool, "/shared", &["EPUB"])
        .await
        .unwrap();
    assert_eq!(ebooks.len(), 1);
    assert_eq!(ebooks[0].uuid, "uuid-epub");
    assert_eq!(ebooks[0].mtime_epoch, 100);
    assert_eq!(ebooks[0].size_bytes, 100);

    let audiobooks = list_indexed_rows_for_formats(&pool, "/shared", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    assert_eq!(audiobooks.len(), 1);
    assert_eq!(audiobooks[0].uuid, "uuid-m4b");
}

// ---------- migration 0024: drop dead book_files.mtime (F19) ----------

#[tokio::test]
async fn migration_drops_book_files_mtime_text_column_but_keeps_mtime_epoch() {
    // F19: the OPF `dcterms:modified` TEXT column was write-only and is
    // dropped by 0024; the filesystem-stat `mtime_epoch` (used by the
    // incremental reindex diff) must survive.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('book_files') WHERE name IN ('mtime', 'mtime_epoch')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        !columns.iter().any(|c| c == "mtime"),
        "book_files.mtime should be dropped by migration 0024"
    );
    assert!(
        columns.iter().any(|c| c == "mtime_epoch"),
        "book_files.mtime_epoch must remain for change detection"
    );
}

#[tokio::test]
async fn list_indexed_rows_for_formats_returns_empty_for_empty_allow_list() {
    // Defensive contract: callers passing an empty allow-list mean
    // "no formats to match against" and must get an empty result, not
    // every row (which would re-introduce the #328 bug).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', '/lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES ('uuid-a', ?, '/lib/a', 'A', 'A') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'a', 0, 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let rows = list_indexed_rows_for_formats(&pool, "/lib", &[])
        .await
        .unwrap();
    assert!(rows.is_empty());
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
async fn get_book_returns_collapsed_primary_file_and_series_for_book_with_series() {
    // Same collapsed projection on the single-book read path (`get_book`
    // shares `BOOK_COLUMNS` verbatim through `row_to_ebook`).
    let _covers = CoversTempDir::new("collapse_get_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "lonely.epub",
            Some("Lonely"),
            &["Author B"],
            &[],
            Some(("Saga", "1")),
            None,
        )],
    )
    .await
    .unwrap();
    let id = list_books(&pool, "/lib").await.unwrap()[0].id;

    let book = get_book(&pool, id).await.unwrap().expect("book exists");
    assert_eq!(book.filename, "lonely.epub");
    assert_eq!(book.series.as_deref(), Some("Saga"));
    assert_eq!(book.series_id, Some(series_id_by_name(&pool, "Saga").await));
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

// ---------- bulk canonical-uuid resolve (issue #633) ----------

/// Insert a `merged_uuids` row the way the merge tx does, so the bulk resolver
/// has a merged-uuid to fall back through.
async fn seed_merged_uuid_for_bulk(pool: &sqlx::SqlitePool, uuid: &str, book_id: i64) {
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, 'epub', '/lib')",
    )
    .bind(uuid)
    .bind(book_id)
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

#[tokio::test]
async fn resolve_canonical_book_uuids_bulk_maps_direct_merged_and_omits_unknown() {
    // Straddles all three cases in one call: a direct `books.uuid`, a merged
    // uuid that resolves to a different canonical, and an unknown uuid that
    // must be **absent** from the returned map (not present with `None`).
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let books = list_books(&pool, "/lib").await.unwrap();
    let direct_uuid = books[0].unique_identifier.clone().unwrap();
    let survivor_id = books[1].id;
    let survivor_uuid = books[1].unique_identifier.clone().unwrap();
    seed_merged_uuid_for_bulk(&pool, "merged-uuid-x", survivor_id).await;

    let batch = vec![
        direct_uuid.clone(),
        "merged-uuid-x".to_string(),
        "no-such-uuid".to_string(),
        direct_uuid.clone(), // dup — bulk resolver must dedup without error
    ];
    let mut tx = pool.begin().await.unwrap();
    let map = resolve_canonical_book_uuids_bulk_exec(&mut tx, &batch)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(map.get(&direct_uuid), Some(&direct_uuid));
    assert_eq!(map.get("merged-uuid-x"), Some(&survivor_uuid));
    assert!(
        !map.contains_key("no-such-uuid"),
        "unknown uuids must be absent from the map (not None)"
    );
    assert_eq!(map.len(), 2, "dedup + skip-unknown: exactly two entries");
}

#[tokio::test]
async fn resolve_canonical_book_uuids_bulk_empty_input_returns_empty_map() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let map = resolve_canonical_book_uuids_bulk_exec(&mut tx, &[])
        .await
        .unwrap();
    assert!(map.is_empty());
}

// ---------- migration 0025: library-scoped landing-sort index (F5) ----------

#[tokio::test]
async fn migration_creates_composite_library_sort_index_for_landing_projection() {
    // F5: the `(library_id, sort, id)` composite index lets the planner seek
    // the library filter and supply `ORDER BY sort, id` without a temp sort.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let exists: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'index' AND name = 'idx_books_library_sort'",
    )
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(exists.as_deref(), Some("idx_books_library_sort"));
}

#[tokio::test]
async fn get_book_uuid_by_scan_key_returns_uuid_for_known_path() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    // `seed_minimal_books` files books under scan_root `/lib` with scan_key
    // `b<i>.epub` and uuid `uuid-<i>`.
    let uuid = get_book_uuid_by_scan_key(&pool, "/lib", "b2.epub")
        .await
        .unwrap();
    assert_eq!(uuid.as_deref(), Some("uuid-2"));
}

#[tokio::test]
async fn get_book_uuid_by_scan_key_returns_none_for_unknown_key() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    // Unknown scan_key under a known root, and a known key under an unknown
    // root, both resolve to None.
    assert!(get_book_uuid_by_scan_key(&pool, "/lib", "missing.epub")
        .await
        .unwrap()
        .is_none());
    assert!(get_book_uuid_by_scan_key(&pool, "/other", "b1.epub")
        .await
        .unwrap()
        .is_none());
}

// ---------- ISBN-13 (issue #1088) ----------

/// Seed one book with a single scanned identifier and return the row's
/// `books.id` alongside its stable uuid.
async fn seed_book_with_identifier(
    pool: &sqlx::SqlitePool,
    scheme: &str,
    value: &str,
) -> (i64, String) {
    replace_books(
        pool,
        "/lib",
        vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "isbn.epub".into(),
                title: Some("Book With Identifier".into()),
                identifiers: vec![Identifier {
                    value: value.into(),
                    scheme: Some(scheme.into()),
                }],
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
    let books = list_books(pool, "/lib").await.unwrap();
    let book = &books[0];
    (book.id, book.unique_identifier.clone().unwrap())
}

#[tokio::test]
async fn get_book_derives_isbn13_from_scanned_isbn_scheme_identifier() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}

#[tokio::test]
async fn get_book_strips_hyphens_when_deriving_isbn13() {
    // OPF ISBN values are commonly hyphenated; the derivation strips
    // non-digit characters before checking the 13-digit length.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "978-0-13-468599-1").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}

#[tokio::test]
async fn get_book_ignores_ten_digit_isbn_when_deriving_isbn13() {
    // An ISBN-10 (10 digits) is a distinct identifier value; it must not be
    // mistaken for an ISBN-13.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "ISBN", "0134685997").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(book.isbn13.is_none());
}

#[tokio::test]
async fn get_book_isbn13_is_none_when_no_isbn_scheme_identifier_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (id, _) = seed_book_with_identifier(&pool, "calibre", "some-opaque-id").await;
    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(book.isbn13.is_none());
}

#[tokio::test]
async fn get_book_isbn13_override_wins_over_scanned_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some("9780316769488".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780316769488"));
}

#[tokio::test]
async fn get_book_isbn13_override_clears_scanned_value_with_empty_string() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "ISBN", "9780134685991").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some(String::new()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert!(
        book.isbn13.is_none(),
        "an empty-string override must clear the scanned ISBN-13, not persist as an empty string"
    );
}

#[tokio::test]
async fn get_book_isbn13_override_applies_when_no_scanned_value_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (id, uuid) = seed_book_with_identifier(&pool, "calibre", "opaque-id").await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            isbn13: Some("9780134685991".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.isbn13.as_deref(), Some("9780134685991"));
}

// ---------- BooksError variants ----------

#[test]
fn books_error_maps_overrides_serialization_to_overrides_json_variant() {
    // The `From<MetadataOverridesError>` bridge (the only site that mints
    // `BooksError::OverridesJson`) must route a corrupt-overrides-JSON
    // deserialization failure to `OverridesJson`, carrying the underlying
    // `serde_json::Error` and its message — never collapsing it into `Db`.
    let json_err =
        serde_json::from_str::<MetadataOverrides>("{ not valid json").expect_err("must not parse");
    let src = crate::metadata_overrides::MetadataOverridesError::Serialization(json_err);
    let err: BooksError = src.into();
    assert!(
        matches!(err, BooksError::OverridesJson(_)),
        "Serialization must map to OverridesJson, got {err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("overrides deserialization failed"),
        "got {err}"
    );
}

// ---------- F Physical Check-In: physical-only visibility (#1181) ----------

async fn seed_fileless(pool: &sqlx::SqlitePool, title: &str, authors: Vec<&str>) -> String {
    crate::physical::create_fileless_book(
        pool,
        crate::physical::FilelessBook {
            title: title.into(),
            authors: authors.into_iter().map(str::to_string).collect(),
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn physical_only_book_is_visible_but_wishlist_only_is_hidden() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await; // one normal /lib book

    // Physical-only: fileless book with a checked-in copy.
    let phys = seed_fileless(&pool, "Physical Only Title", vec!["Print Author"]).await;
    crate::physical::add_physical_copy(&pool, &phys, None, None, None)
        .await
        .unwrap();
    // Wishlist-only: fileless book, no physical copy.
    let wish = seed_fileless(&pool, "Wishlist Only Title", vec![]).await;

    let list = list_books(&pool, "/lib").await.unwrap();
    let uuids: Vec<&str> = list
        .iter()
        .filter_map(|b| b.unique_identifier.as_deref())
        .collect();
    assert!(
        uuids.contains(&phys.as_str()),
        "physical-only book must appear"
    );
    assert!(
        !uuids.contains(&wish.as_str()),
        "wishlist-only book must be hidden"
    );

    // AC3: the physical flag is set (drives the badge).
    let phys_row = list
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&phys))
        .unwrap();
    assert!(phys_row.has_physical);

    // AC1: searchable by title.
    let hits = search_books(&pool, "/lib", "Physical Only").await.unwrap();
    assert!(hits
        .iter()
        .any(|b| b.unique_identifier.as_deref() == Some(&phys)));
    // AC2: wishlist-only book is not searchable.
    let miss = search_books(&pool, "/lib", "Wishlist Only").await.unwrap();
    assert!(miss.is_empty());

    // AC1: the physical-only book contributes to sidebar facets even though it
    // lives under the synthetic `physical://local` root, not `/lib`.
    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert!(
        facets.authors.iter().any(|f| f.value == "Print Author"),
        "physical-only book's author must appear in facets"
    );
}

#[tokio::test]
async fn get_book_files_publishes_a_content_validator_derived_from_the_stat() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let files = get_book_files(&pool, book_id).await.unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "the wire validator is the (mtime_epoch, size_bytes) pair the reindex diff keys on"
    );
}

#[tokio::test]
async fn get_book_files_omits_the_validator_for_a_row_the_scanner_has_not_stat_ed() {
    // `(0, 0)` is the indexer's never-observed sentinel. Publishing a
    // validator for it would make the one-time stat backfill look like a
    // content change on every device holding a download.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 0, 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let files = get_book_files(&pool, book_id).await.unwrap();
    assert_eq!(files[0].etag, None);
}

#[tokio::test]
async fn get_book_publishes_the_validator_for_an_ordinary_single_file_book() {
    // The case that matters most and was previously omitted: `book_files`
    // used to be withheld unless some format had more than one row, so a
    // typical one-EPUB book reached clients with no validator at all — and
    // a whole library of those is the normal shape. Every offline staleness
    // check reads this field, so withholding it here disables the feature
    // for exactly the common case.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    assert_eq!(
        book.book_files.len(),
        1,
        "a single-file book still lists its file"
    );
    assert_eq!(book.book_files[0].etag.as_deref(), Some("\"ff-1000\""));
}

#[tokio::test]
async fn get_book_publishes_a_validator_per_file_on_a_dual_format_book() {
    // One EPUB + one M4B is still one row per format, so this was withheld
    // too — leaving a dual-format book unable to detect staleness on either
    // of its downloads.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    for (format, size, mtime) in [("EPUB", 4096, 255), ("M4B", 8192, 511)] {
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
             VALUES (?, ?, 'b', ?, ?)",
        )
        .bind(book_id)
        .bind(format)
        .bind(size)
        .bind(mtime)
        .execute(&pool)
        .await
        .unwrap();
    }

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    assert_eq!(book.book_files.len(), 2);
    assert!(
        book.book_files.iter().all(|f| f.etag.is_some()),
        "each format's download needs its own validator"
    );
}

#[tokio::test]
async fn get_book_serializes_the_validator_onto_the_wire() {
    // The projection carrying an etag is not the same as clients receiving
    // one — `book_files` is `skip_serializing_if = "Vec::is_empty"`, and the
    // etag itself is skipped when absent. Pin the actual JSON.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk', 'b.epub', 1, '/lib/b.epub', 'Book') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'b', 4096, 255)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let book = get_book(&pool, book_id).await.unwrap().expect("book");
    let wire: serde_json::Value = serde_json::to_value(&book).unwrap();
    assert_eq!(
        wire["book_files"][0]["etag"].as_str(),
        Some("\"ff-1000\""),
        "clients read the validator off this exact path"
    );
}

/// Seed one book with the given `(format, ordinal, size, mtime)` files.
async fn seed_book_with_files(
    pool: &sqlx::SqlitePool,
    uuid: &str,
    files: &[(&str, i64, i64, i64)],
) -> i64 {
    sqlx::query(
        "INSERT OR IGNORE INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')",
    )
    .execute(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES (?, ?, 1, '/lib/b', 'Book') RETURNING id",
    )
    .bind(uuid)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    for (format, ordinal, size, mtime) in files {
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes, mtime_epoch) \
             VALUES (?, ?, 'b', ?, ?, ?)",
        )
        .bind(book_id)
        .bind(*format)
        .bind(*ordinal)
        .bind(*size)
        .bind(*mtime)
        .execute(pool)
        .await
        .unwrap();
    }
    book_id
}

fn validator_query(
    uuid: &str,
    format: omnibus_shared::DownloadFormat,
    file_id: Option<i64>,
) -> omnibus_shared::DownloadValidatorQuery {
    omnibus_shared::DownloadValidatorQuery {
        book_uuid: uuid.into(),
        format,
        file_id,
    }
}

#[tokio::test]
async fn download_validators_answers_each_query_about_the_file_the_server_would_serve() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Two EPUB editions and two audio parts, so "which row" is a real
    // question rather than the only row present.
    seed_book_with_files(
        &pool,
        "bk",
        &[
            ("EPUB", 1, 4096, 255),
            ("EPUB", 0, 8192, 511),
            ("M4B", 1, 1024, 15),
            ("M4B", 0, 2048, 31),
        ],
    )
    .await;

    let answers = download_validators(
        &pool,
        &[
            validator_query("bk", DownloadFormat::Epub, None),
            validator_query("bk", DownloadFormat::Audio, None),
        ],
    )
    .await
    .unwrap();

    // Lowest ordinal of the format, matching `book_file_path`'s
    // `ORDER BY bf.ordinal LIMIT 1` — answering about the other edition
    // would report a download stale that isn't.
    assert_eq!(answers[0].etag.as_deref(), Some("\"1ff-2000\""));
    assert_eq!(answers[1].etag.as_deref(), Some("\"1f-800\""));
}

#[tokio::test]
async fn download_validators_honours_an_explicitly_chosen_file() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book_with_files(
        &pool,
        "bk",
        &[("EPUB", 0, 8192, 511), ("EPUB", 1, 4096, 255)],
    )
    .await;
    let second: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND ordinal = 1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();

    let answers = download_validators(
        &pool,
        &[validator_query("bk", DownloadFormat::Epub, Some(second))],
    )
    .await
    .unwrap();

    assert_eq!(
        answers[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "an explicit file_id must win over the default row"
    );
}

#[tokio::test]
async fn download_validators_reports_no_etag_for_anything_it_cannot_answer() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_book_with_files(&pool, "bk", &[("EPUB", 0, 4096, 255), ("M4B", 0, 0, 0)]).await;

    let answers = download_validators(
        &pool,
        &[
            // Unknown book.
            validator_query("gone", DownloadFormat::Epub, None),
            // Known book, but no file of that format.
            validator_query("bk", DownloadFormat::Audio, Some(9999)),
            // Known file the scanner has never stat'd — the (0, 0) sentinel.
            validator_query("bk", DownloadFormat::Audio, None),
        ],
    )
    .await
    .unwrap();

    assert!(answers.iter().all(|a| a.etag.is_none()));
    // The shape still round-trips, so a client can line answers up with the
    // questions it asked.
    assert_eq!(answers[0].book_uuid, "gone");
    assert_eq!(answers[1].file_id, Some(9999));
}

#[tokio::test]
async fn download_validators_answers_a_batch_in_order() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    for (uuid, mtime) in [("bk-1", 255), ("bk-2", 511), ("bk-3", 767)] {
        seed_book_with_files(&pool, uuid, &[("EPUB", 0, 4096, mtime)]).await;
    }

    let queries: Vec<_> = ["bk-3", "bk-1", "bk-2"]
        .iter()
        .map(|u| validator_query(u, DownloadFormat::Epub, None))
        .collect();
    let answers = download_validators(&pool, &queries).await.unwrap();

    let uuids: Vec<&str> = answers.iter().map(|a| a.book_uuid.as_str()).collect();
    assert_eq!(
        uuids,
        ["bk-3", "bk-1", "bk-2"],
        "answers ride with their questions"
    );
    assert_eq!(answers[0].etag.as_deref(), Some("\"2ff-1000\""));
}

#[tokio::test]
async fn download_validators_resolves_a_merged_uuid_to_the_surviving_book() {
    use omnibus_shared::DownloadFormat;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let book_id = seed_book_with_files(&pool, "bk", &[("EPUB", 0, 4096, 255)]).await;
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key) \
         VALUES ('old-uuid', ?, 'EPUB', '/lib', 'b.epub')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let answers = download_validators(
        &pool,
        &[validator_query("old-uuid", DownloadFormat::Epub, None)],
    )
    .await
    .unwrap();

    assert_eq!(
        answers[0].etag.as_deref(),
        Some("\"ff-1000\""),
        "a download taken before a merge must still be answerable"
    );
}
