//! `get_book` and its per-book scalars: the machine-readable format
//! timestamps, `book_last_modified_for`, `book_display_title`, the collapsed
//! primary file, and description sanitization.

use omnibus_shared::EbookMetadata;

use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, seed_minimal_books, series_id_by_name, CoversTempDir};

use super::super::*;

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
async fn book_display_title_returns_title_and_falls_back_to_scan_key_when_empty() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_display_title(&pool, id).await.unwrap().as_deref(),
        Some("Title 1")
    );

    // NULL and empty titles both fall back to the library-relative scan_key.
    sqlx::query("UPDATE books SET title = '' WHERE uuid = 'uuid-1'")
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_display_title(&pool, id).await.unwrap().as_deref(),
        Some("b1.epub")
    );
}

#[tokio::test]
async fn book_display_title_returns_none_for_unknown_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert_eq!(book_display_title(&pool, 999).await.unwrap(), None);
}

#[tokio::test]
async fn book_display_title_by_uuid_resolves_and_returns_none_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    assert_eq!(
        book_display_title_by_uuid(&pool, "uuid-1")
            .await
            .unwrap()
            .as_deref(),
        Some("Title 1")
    );
    assert_eq!(
        book_display_title_by_uuid(&pool, "no-such-uuid")
            .await
            .unwrap(),
        None
    );
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
