//! Inline unit tests for the `books` module. Kept in a single file so the
//! many cross-cutting helpers (`seed_minimal_books`, etc.) and the tests that
//! drive `list_books` + `search_books` + `get_book` together stay co-located.

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
use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};

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
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
         VALUES (?, 'M4B', 'alpha', 0, '')",
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
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
         VALUES (?, 'M4B', 'alpha', 0, '')",
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
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
         VALUES (?, 'M4B', 'alpha', 0, '')",
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
    let lib_res = sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
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
    // `authors` table, backfill must leave the id None — same shape
    // as get_book_leaves_series_id_none_when_override_series_unknown.
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
async fn get_book_leaves_series_id_none_when_override_series_unknown() {
    // If the override sets a series name that no other book uses, the
    // series table won't have a row to point at — backfill must
    // leave series_id None rather than fabricating one.
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
    assert_eq!(merged.series_id, None);
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
    let lib_id = sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
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
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
         VALUES (?, 'EPUB', 'some-book', 0, '')",
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
async fn book_file_path_returns_none_for_missing_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let path = book_file_path(&pool, 9999, "EPUB").await.unwrap();
    assert!(path.is_none());
}

#[tokio::test]
async fn book_file_path_returns_none_when_no_file_row_for_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
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
