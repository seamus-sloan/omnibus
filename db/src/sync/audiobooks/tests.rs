//! Direct unit tests for the `sync/audiobooks` write-path helpers. Mirrors
//! `sync/books/tests.rs`'s shape: each bucket helper and shared row writer
//! is exercised against an in-memory DB — the integration-style
//! `sync_audiobooks` tests in `sync/tests.rs` cover only the composed happy
//! path, not these per-helper contracts (attach, re-attach, backfill).

use std::collections::HashSet;

use sqlx::SqlitePool;

use super::backfill_audiobook_stats;
use super::shared::{attach_audiobook_file, insert_new_audiobook, try_attach_new_audiobook};
use super::stamp_audiobooks_last_indexed;
use super::{
    sync_audiobooks, sync_audiobooks_changed, sync_audiobooks_new, sync_audiobooks_removed,
    AudiobookSyncPlan,
};
use crate::pool::init_db;
use crate::sync::books::SyncError;
use crate::test_support::{count_rows, indexed_audiobook, seed_synced_ebook, CoversTempDir};

/// Insert a `scan_roots` row for `/lib` and return its id — the
/// `library_id` every bucket helper needs.
async fn seed_scan_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed one audiobook (canonical `books` row + its `book_files`/parts/
/// chapters/author-link rows) through the real write helper, returning its
/// `books.id`. Local to the test module so production code stays lean.
async fn seed_audiobook_with_file(pool: &SqlitePool, library_id: i64, group_path: &str) -> i64 {
    let b = indexed_audiobook(group_path, "Seeded", Some("Seed Author"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    sqlx::query_scalar("SELECT id FROM books WHERE scan_key = ?")
        .bind(group_path)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// COUNT `book_files` rows for one `book_id`.
async fn book_files_count(pool: &SqlitePool, book_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ── sync_audiobooks_new ──────────────────────────────────────────────

/// `sync_audiobooks_new` inserts a brand-new audiobook: a canonical `books`
/// row, its `book_files` row, its parts, a synthesized chapter (no chapter
/// markers supplied), and the author link.
#[tokio::test]
async fn sync_audiobooks_new_inserts_a_new_audiobook_and_its_rows() {
    let _covers = CoversTempDir::new("ab_sync_new_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let new_books = vec![indexed_audiobook(
        "Author/Fresh.m4b",
        "Fresh",
        Some("Author"),
    )];
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_audiobooks_new(&mut tx, library_id, "/lib", &new_books, &[], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(covers.is_empty(), "no cover was supplied");
    let book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'Author/Fresh.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Fresh");
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "book_files inserted"
    );
    let book_file_id: i64 = sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = {book_file_id}")
        )
        .await,
        1,
        "one part inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM file_chapters WHERE book_file_id = {book_file_id}")
        )
        .await,
        1,
        "no chapter markers supplied, so one chapter is synthesized per part"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1,
        "author link inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_fts WHERE rowid = {book_id}")
        )
        .await,
        1,
        "FTS row inserted"
    );
}

/// `sync_audiobooks_new` rewrites a same-scan_key row in place — the
/// fileless-book-whose-group-returned path — preserving `books.id`/`uuid`
/// rather than inserting a duplicate.
#[tokio::test]
async fn sync_audiobooks_new_rewrites_a_fileless_book_in_place_when_scan_key_matches() {
    let _covers = CoversTempDir::new("ab_sync_new_rewrite");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // The group's file disappeared, so the diff would mark it Removed …
    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, std::slice::from_ref(&uuid_before))
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        book_files_count(&pool, book_id).await,
        0,
        "file removed, book now fileless"
    );

    // … and returned, so the next scan classifies the same scan_key as New.
    let returned = indexed_audiobook("Author/Book.m4b", "Book", Some("Seed Author"));
    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_new(&mut tx, library_id, "/lib", &[returned], &[], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "rewrite updates in place, not insert"
    );
    let (id_after, uuid_after): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Author/Book.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "books.id preserved across the rewrite");
    assert_eq!(
        uuid_after, uuid_before,
        "books.uuid preserved across the rewrite"
    );
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "book_files re-created"
    );
}

/// `sync_audiobooks_new` with an empty batch is a no-op and returns no
/// cover triples (early return before the id pre-fetch).
#[tokio::test]
async fn sync_audiobooks_new_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_audiobooks_new(&mut tx, library_id, "/lib", &[], &[], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(covers.is_empty());
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
}

// ── sync_audiobooks_changed ──────────────────────────────────────────

/// `sync_audiobooks_changed` updates an existing audiobook in place — the
/// `books.id`/`uuid` are preserved while the scalars, parts, chapters, and
/// author link are wiped and rewritten from the fresh parse.
#[tokio::test]
async fn sync_audiobooks_changed_updates_an_existing_audiobook_row_in_place() {
    let _covers = CoversTempDir::new("ab_sync_changed_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Same scan_key (→ same group), new title + author.
    let mut changed = indexed_audiobook("Author/Book.m4b", "Updated Title", Some("New Author"));
    changed.parts[0].duration_seconds = 7200.0;
    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_changed(&mut tx, library_id, "/lib", &[changed], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "changed updates in place, not insert"
    );
    let (id_after, uuid_after, title_after): (i64, String, String) =
        sqlx::query_as("SELECT id, uuid, title FROM books WHERE scan_key = 'Author/Book.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "books.id preserved across change");
    assert_eq!(
        uuid_after, uuid_before,
        "books.uuid preserved across change"
    );
    assert_eq!(title_after, "Updated Title", "scalar columns refreshed");

    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM authors a
         JOIN books_authors_link l ON l.author = a.id
         WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        authors,
        vec!["New Author".to_string()],
        "author link rewritten"
    );

    let duration: f64 = sqlx::query_scalar(
        "SELECT p.duration_seconds FROM book_file_parts p
         JOIN book_files f ON f.id = p.book_file_id
         WHERE f.book_id = ?",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        duration, 7200.0,
        "parts wiped and re-inserted from the fresh parse"
    );
}

/// `sync_audiobooks_changed` promotes a TOCTOU miss to a New insert: the
/// diff said this scan_key existed, but no `books` row (nor a
/// `merged_uuids` attachment) is found for it at write time.
#[tokio::test]
async fn sync_audiobooks_changed_promotes_to_new_insert_on_toctou_miss() {
    let _covers = CoversTempDir::new("ab_sync_changed_toctou");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let phantom = indexed_audiobook("Author/Phantom.m4b", "Phantom", Some("Author"));
    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_changed(&mut tx, library_id, "/lib", &[phantom], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "the missing scan_key is promoted to a New insert"
    );
    let title: String = sqlx::query_scalar("SELECT title FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Phantom");
}

/// `sync_audiobooks_changed` refreshes an *attached* file's `book_files` row
/// (via `merged_uuids`) rather than creating a second `books` row, when the
/// changed scan_key has no primary `books` row of its own.
#[tokio::test]
async fn sync_audiobooks_changed_refreshes_attached_file_via_merged_uuids_ledger() {
    let _covers = CoversTempDir::new("ab_sync_changed_attach");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let library_id: i64 = sqlx::query_scalar("SELECT library_id FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "attach adds a book_files row, not a second books row"
    );

    let mut refreshed = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    refreshed.parts[0].duration_seconds = 9999.0;
    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_changed(&mut tx, library_id, "/lib", &[refreshed], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "still one books row — the changed part refreshed the attachment"
    );
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        title, "Dracula",
        "target metadata untouched by the attach refresh"
    );
    let duration: f64 = sqlx::query_scalar(
        "SELECT p.duration_seconds FROM book_file_parts p
         JOIN book_files f ON f.id = p.book_file_id
         WHERE f.book_id = ? AND f.format = 'M4B'",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        duration, 9999.0,
        "the attached file's own row was refreshed"
    );
}

/// `sync_audiobooks_changed` with an empty batch is a no-op and returns no
/// cover triples (early return before the id pre-fetch).
#[tokio::test]
async fn sync_audiobooks_changed_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_audiobooks_changed(&mut tx, library_id, "/lib", &[], |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(covers.is_empty());
}

// ── sync_audiobooks_removed ───────────────────────────────────────────

/// `sync_audiobooks_removed` ghosts an audiobook: its `book_files` row (and
/// cascading parts/chapters) is dropped, but the durable `books` row
/// survives so the uuid — and any user data keyed on it — persists.
#[tokio::test]
async fn sync_audiobooks_removed_retains_books_row_and_removes_book_files_row() {
    let _covers = CoversTempDir::new("ab_sync_removed_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let book_file_id: i64 = sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, &[uuid])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let books_still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_still, 1,
        "the books row is retained (durable identity)"
    );
    assert_eq!(
        book_files_count(&pool, book_id).await,
        0,
        "the book_files row is removed"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = {book_file_id}")
        )
        .await,
        0,
        "parts cascade off the deleted book_files row"
    );
    let flagged: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flagged, 1, "retained row is flagged missing");
}

/// A book whose own audiobook group is removed, but which still holds a
/// cross-format attachment (a different format's file, recorded in
/// `merged_uuids` and still present), is not flagged missing — the
/// surviving file means the book isn't actually fileless.
#[tokio::test]
async fn sync_audiobooks_removed_does_not_flag_missing_when_a_cross_format_attachment_survives() {
    let _covers = CoversTempDir::new("ab_sync_removed_survives_attach");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Forge a surviving cross-format attachment: a book_files row in a
    // different format, backed by its own merged_uuids ledger entry.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'EPUB', 'Book', 1000, 100, 'Author/Book.epub')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('attached-epub', ?, 'EPUB', '/ebooks', 'Author/Book.epub')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, &[uuid])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // The M4B row is dropped, the EPUB survives, and the book must not be
    // flagged missing since it still holds a file.
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        0,
        "the book's own native group row is dropped"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        1,
        "the cross-format attachment survives"
    );
    let is_missing: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        is_missing, 0,
        "a surviving cross-format attachment means the book isn't fileless"
    );
}

/// One `sync_audiobooks_removed` call ghosts every uuid in the batch — the
/// batched resolve + `mark_book_files_missing_batch` covers N groups in a
/// single invocation.
#[tokio::test]
async fn sync_audiobooks_removed_ghosts_multiple_groups_in_one_call() {
    let _covers = CoversTempDir::new("ab_sync_removed_multi");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut uuids = Vec::new();
    for i in 0..3 {
        let book_id =
            seed_audiobook_with_file(&pool, library_id, &format!("Author/B{i}.m4b")).await;
        let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        uuids.push(uuid);
    }
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        3
    );

    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, &uuids)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 3);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books WHERE is_missing_files = 1"
        )
        .await,
        3
    );
}

/// A removed uuid that only lived in `merged_uuids` (a cross-format
/// attachment, no `books` row of its own) drops its `book_files` row and
/// ledger entry via `attach::remove_attached_files`, leaving the target
/// book intact.
#[tokio::test]
async fn sync_audiobooks_removed_drops_attached_file_recorded_only_in_merged_uuids() {
    let _covers = CoversTempDir::new("ab_sync_removed_attach");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let library_id: i64 = sqlx::query_scalar("SELECT library_id FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let attached_uuid: String =
        sqlx::query_scalar("SELECT uuid FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );

    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, &[attached_uuid])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "the target book survives"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        1,
        "only the attached M4B file row is dropped"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "the ledger entry is dropped alongside the file"
    );
}

/// `sync_audiobooks_removed` is a no-op for an empty batch.
#[tokio::test]
async fn sync_audiobooks_removed_is_a_noop_for_empty_batch() {
    let _covers = CoversTempDir::new("ab_sync_removed_noop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Keep.m4b").await;

    let mut tx = pool.begin().await.unwrap();
    sync_audiobooks_removed(&mut tx, library_id, &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "empty removed batch leaves the file row intact"
    );
}

// ── insert_new_audiobook ──────────────────────────────────────────────

/// `insert_new_audiobook` writes the canonical `books`/`book_files` rows
/// plus parts, chapters, and the author link, and pushes the cover triple
/// when the group carries one.
#[tokio::test]
async fn insert_new_audiobook_writes_books_book_files_parts_chapters_and_author_link() {
    let _covers = CoversTempDir::new("ab_insert_new_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut b = indexed_audiobook("Author/Solo.m4b", "Solo", Some("Solo Author"));
    b.cover = Some(("image/png".into(), vec![1, 2, 3]));

    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(covers.len(), 1, "the cover triple is pushed");
    let book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'Author/Solo.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(book_files_count(&pool, book_id).await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1
    );
}

/// `insert_new_audiobook` propagates the underlying `sqlx::Error` as
/// `SyncError::Db` when a downstream write (here, the parts insert) hits a
/// missing table.
#[tokio::test]
async fn insert_new_audiobook_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let b = indexed_audiobook("Author/Broken.m4b", "Broken", Some("Author"));

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_file_parts")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut covers = Vec::new();
    let err = insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

// ── try_attach_new_audiobook ──────────────────────────────────────────

/// `try_attach_new_audiobook` attaches unconditionally when the file's
/// `scan_key` is already recorded in `merged_uuids` — even though titles no
/// longer need to match, since the ledger hit is checked first.
#[tokio::test]
async fn try_attach_new_audiobook_attaches_via_recorded_ledger_entry() {
    let _covers = CoversTempDir::new("ab_try_attach_ledger");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Stoker/Dracula.m4b')",
    )
    .bind(target_id)
    .execute(&pool)
    .await
    .unwrap();

    // Title no longer matches — the ledger hit must attach anyway.
    let b = indexed_audiobook(
        "Stoker/Dracula.m4b",
        "A Totally Different Title",
        Some("Someone Else"),
    );
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        attached,
        "the ledger entry attaches regardless of title drift"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no second books row was created"
    );
    assert_eq!(book_files_count(&pool, target_id).await, 2);
}

/// `try_attach_new_audiobook` attaches via the title+author heuristic when
/// exactly one existing book matches and lacks this format — the
/// no-ledger-entry path.
#[tokio::test]
async fn try_attach_new_audiobook_attaches_via_title_author_match_when_no_ledger_entry() {
    let _covers = CoversTempDir::new("ab_try_attach_heuristic");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(attached);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(book_files_count(&pool, target_id).await, 2);
}

/// `try_attach_new_audiobook` returns `Ok(false)` without writing when no
/// author is present — too weak a signal to auto-match, per the doc
/// comment.
#[tokio::test]
async fn try_attach_new_audiobook_declines_when_no_author_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let b = indexed_audiobook("Author/NoAuthor.m4b", "No Author", None);
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!attached);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
}

/// `try_attach_new_audiobook` propagates a `sqlx::Error` from the
/// title+author lookup (`find_attach_target`, which joins `book_files`) as
/// `SyncError::Db`.
#[tokio::test]
async fn try_attach_new_audiobook_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let b = indexed_audiobook("Author/Broken.m4b", "Broken", Some("Author"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_files")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

// ── attach_audiobook_file ─────────────────────────────────────────────

/// `attach_audiobook_file` writes the attached file's `book_files`/parts/
/// chapters rows under the target book, records the `merged_uuids` ledger
/// row, and adopts the cover when the target has none.
#[tokio::test]
async fn attach_audiobook_file_writes_rows_and_ledger_and_adopts_cover() {
    let _covers = CoversTempDir::new("ab_attach_file_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    b.cover = Some(("image/png".into(), vec![9, 9, 9]));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(book_files_count(&pool, target_id).await, 2);
    let (mu_book, mu_lib): (i64, String) =
        sqlx::query_as("SELECT book_id, library_path FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mu_book, target_id);
    assert_eq!(mu_lib, "/lib");
    let book_file_id: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(target_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = {book_file_id}")
        )
        .await,
        1
    );
    assert_eq!(
        covers.len(),
        1,
        "the target had no cover, so the attached file's cover is adopted"
    );
}

/// Re-attaching a part of an already-attached multi-part file (a stat
/// change) preserves its prior `ordinal` slot rather than jumping to the
/// end of the picker order: with 3 attached M4B parts (ordinals 0,1,2),
/// re-attaching the middle one must not let `COALESCE(MAX(ordinal)+1, ...)`
/// recompute against only the surviving siblings (which would yield 3, not 1).
#[tokio::test]
async fn attach_audiobook_file_preserves_prior_ordinal_on_reattach() {
    let _covers = CoversTempDir::new("ab_attach_file_ordinal");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    for part in [
        "Stoker/Dracula-1.m4b",
        "Stoker/Dracula-2.m4b",
        "Stoker/Dracula-3.m4b",
    ] {
        let b = indexed_audiobook(part, "Dracula", Some("Bram Stoker"));
        let mut covers = Vec::new();
        let mut tx = pool.begin().await.unwrap();
        attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let ordinal_before: i64 = sqlx::query_scalar(
        "SELECT ordinal FROM book_files
          WHERE book_id = ? AND format = 'M4B' AND scan_key = 'Stoker/Dracula-2.m4b'",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ordinal_before, 1, "the middle part holds slot 1");

    // Re-attach the middle part (e.g. a stat change) — its ordinal slot
    // must be preserved, not recomputed against the surviving siblings.
    let mut re_b = indexed_audiobook("Stoker/Dracula-2.m4b", "Dracula", Some("Bram Stoker"));
    re_b.max_mtime_epoch = 999;
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &re_b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let ordinal_after: i64 = sqlx::query_scalar(
        "SELECT ordinal FROM book_files
          WHERE book_id = ? AND format = 'M4B' AND scan_key = 'Stoker/Dracula-2.m4b'",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        ordinal_after, ordinal_before,
        "the middle slot survives re-attach rather than jumping to the end"
    );
}

/// `attach_audiobook_file` propagates a `sqlx::Error` as `SyncError::Db`
/// when the parts insert hits a missing table.
#[tokio::test]
async fn attach_audiobook_file_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_file_parts")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

// ── backfill_audiobook_stats / stamp_audiobooks_last_indexed ─────────

/// `backfill_audiobook_stats` UPDATEs only the `book_files.(mtime_epoch,
/// size_bytes)` columns for the given uuid — no re-parse, no link writes.
#[tokio::test]
async fn backfill_audiobook_stats_updates_mtime_and_size_for_matching_uuid() {
    let _covers = CoversTempDir::new("ab_backfill_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    backfill_audiobook_stats(&mut tx, library_id, &[(uuid, 555, 777)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (mtime, size): (i64, i64) =
        sqlx::query_as("SELECT mtime_epoch, size_bytes FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mtime, 555);
    assert_eq!(size, 777);
}

/// `backfill_audiobook_stats` is a no-op for an empty batch.
#[tokio::test]
async fn backfill_audiobook_stats_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    backfill_audiobook_stats(&mut tx, library_id, &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// `stamp_audiobooks_last_indexed` stamps `scan_roots.last_indexed` with a
/// non-zero wall-clock value.
#[tokio::test]
async fn stamp_audiobooks_last_indexed_sets_scan_roots_last_indexed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let before: Option<i64> =
        sqlx::query_scalar("SELECT last_indexed FROM scan_roots WHERE id = ?")
            .bind(library_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, None, "unstamped before the call");

    let mut tx = pool.begin().await.unwrap();
    stamp_audiobooks_last_indexed(&mut tx, library_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let after: Option<i64> = sqlx::query_scalar("SELECT last_indexed FROM scan_roots WHERE id = ?")
        .bind(library_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        after.unwrap_or(0) > 0,
        "last_indexed stamped with wall-clock seconds"
    );
}

// ── Moved/renamed audiobook group relocation ────────────────────────────

/// AC6: AC1-AC4 hold for an audiobook group directory moved within the
/// audiobook library. The moved group is classified Removed (old scan_key)
/// and New (new scan_key) in the same reindex plan; the relocation must be
/// recognized and rewritten in place — updating `books.scan_key` (AC1),
/// preserving `books.id`/`books.uuid` (AC4), clearing `is_missing_files`
/// (AC2), and minting no `merged_uuids` row (AC3) — instead of being
/// re-bound as a cross-format attachment onto its own just-vacated slot.
#[tokio::test]
async fn moved_audiobook_group_relocates_in_place_instead_of_minting_a_merged_uuids_row() {
    let _covers = CoversTempDir::new("ab_sync_relocation");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Old/Book").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // One reindex pass observing the move: the old scan_key is Removed, the
    // new group directory is New — exactly what the real diff produces.
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("New/Book", "Seeded", Some("Seed Author"))],
            removed_uuids: vec![uuid_before.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no duplicate book was created"
    );
    let (id_after, uuid_after, scan_key_after, is_missing): (i64, String, String, i64) =
        sqlx::query_as("SELECT id, uuid, scan_key, is_missing_files FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "AC4: books.id preserved");
    assert_eq!(uuid_after, uuid_before, "AC4: books.uuid preserved");
    assert_eq!(scan_key_after, "New/Book", "AC1: scan_key follows the move");
    assert_eq!(
        is_missing, 0,
        "AC2: not flagged missing after the relocation"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "AC3: no cross-format ledger row minted for a same-format relocation"
    );
    assert_eq!(book_files_count(&pool, book_id).await, 1, "one file row");
}

// ── #1536: the Moved bucket (stat-pair relocation writer) ────────────

/// Ordered `book_file_parts.filename` values under one book.
async fn part_filenames(pool: &SqlitePool, book_id: i64) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT p.filename FROM book_file_parts p
           JOIN book_files bf ON bf.id = p.book_file_id
          WHERE bf.book_id = ?
          ORDER BY p.ordinal",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A three-part MP3 group: every part filename is library-relative and
/// sits under the group directory, so a move has to rewrite all three.
fn multipart_mp3_group(group_path: &str) -> crate::audiobook::IndexedAudiobook {
    let mut b = indexed_audiobook(group_path, "Seeded", Some("Seed Author"));
    b.format = "MP3".into();
    b.parts = (0..3)
        .map(|i| crate::audiobook::AudiobookPart {
            ordinal: i,
            filename: format!("{group_path}/0{}.mp3", i + 1),
            size_bytes: 1000,
            mtime_epoch: 100,
            duration_seconds: 1200.0,
        })
        .collect();
    b.total_size_bytes = 3000;
    b
}

/// AC9: a moved audiobook group directory relocates every path column,
/// including each `book_file_parts.filename` — the HLS and download read
/// paths resolve part filenames against the audio root, so a stale part
/// path is a broken book even though `books` looks correct.
#[tokio::test]
async fn moved_audiobook_group_rewrites_every_part_filename() {
    let _covers = CoversTempDir::new("ab_moved_parts");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(
        &mut tx,
        library_id,
        &multipart_mp3_group("Old/Book"),
        &mut covers,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (book_id, uuid): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Old/Book'")
            .fetch_one(&pool)
            .await
            .unwrap();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: uuid.clone(),
                filename: "New/Home/Book".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (scan_key, path, uuid_after): (String, String, String) =
        sqlx::query_as("SELECT scan_key, path, uuid FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(scan_key, "New/Home/Book", "AC9: books.scan_key moved");
    assert_eq!(path, "New/Home", "AC9: books.path is the group's parent");
    assert_eq!(uuid_after, uuid, "identity preserved");

    let (filename, file_scan_key): (String, String) =
        sqlx::query_as("SELECT filename, scan_key FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        filename, "Book",
        "book_files.filename is the group leaf stem"
    );
    assert_eq!(file_scan_key, "New/Home/Book");

    assert_eq!(
        part_filenames(&pool, book_id).await,
        vec![
            "New/Home/Book/01.mp3".to_string(),
            "New/Home/Book/02.mp3".to_string(),
            "New/Home/Book/03.mp3".to_string(),
        ],
        "AC9: every part path is rebased onto the new group directory"
    );
}

/// The single-file case: an `.m4b` group's one part *is* the group path,
/// so the rebase has to handle an empty suffix.
#[tokio::test]
async fn moved_single_file_audiobook_rewrites_its_one_part_path() {
    let _covers = CoversTempDir::new("ab_moved_single");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut b = indexed_audiobook("Old/Book.m4b", "Seeded", Some("Seed Author"));
    b.parts[0].filename = "Old/Book.m4b".into();
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (book_id, uuid): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Old/Book.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid,
                filename: "New/Book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        part_filenames(&pool, book_id).await,
        vec!["New/Book.m4b".to_string()]
    );
    let filename: String = sqlx::query_scalar("SELECT filename FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(filename, "Book", "the extension is stripped for the stem");
}

/// AC9, second half: three consecutive reindexes after the move converge
/// with `is_missing_files = 0` throughout, driven by the real classifier
/// against a fixed on-disk state.
#[tokio::test]
async fn three_reindexes_after_a_moved_audiobook_group_converge_without_ghosting() {
    let _covers = CoversTempDir::new("ab_moved_convergence");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Old/Book").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // What `project_groups_to_stat` hands the classifier for the moved
    // group: the group path as both filename and scan_key, carrying the
    // group's summed size and max mtime.
    let disk = [crate::ebook::StatEntry {
        filename: "New/Book".into(),
        scan_key: "New/Book".into(),
        mtime_epoch: 100,
        size_bytes: 1000,
        error: None,
    }];

    for pass in 0..3 {
        let db_rows = crate::books::list_indexed_rows_for_formats(&pool, "/lib", &["M4B"])
            .await
            .unwrap();
        let diff =
            crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
        if pass == 0 {
            assert_eq!(diff.moved.len(), 1, "pass 0: the group move is detected");
            assert!(diff.removed.is_empty(), "AC9: the group never ghosts");
            assert!(diff.new.is_empty(), "AC9: no Phase-B parse target");
        } else {
            assert!(diff.moved.is_empty(), "pass {pass}: already settled");
            assert_eq!(diff.unchanged.len(), 1, "pass {pass}: classifies Unchanged");
        }

        sync_audiobooks(
            &pool,
            "/lib",
            AudiobookSyncPlan {
                moved: diff.moved,
                removed_uuids: diff.removed,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(book_files_count(&pool, book_id).await, 1, "pass {pass}");
        let (uuid, scan_key, is_missing): (String, String, i64) =
            sqlx::query_as("SELECT uuid, scan_key, is_missing_files FROM books WHERE id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(uuid, uuid_before, "pass {pass}: uuid preserved");
        assert_eq!(scan_key, "New/Book", "pass {pass}: scan_key settled");
        assert_eq!(
            is_missing, 0,
            "AC9: is_missing_files stays 0 on pass {pass}"
        );
        assert_eq!(
            part_filenames(&pool, book_id).await,
            vec!["New/Book/part1.m4b".to_string()],
            "pass {pass}: the part path stays rebased"
        );
        assert_eq!(
            count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
            0,
            "pass {pass}: no ledger row minted"
        );
    }
}

/// End-to-end on the regression the format guard exists for: retyping
/// `Book.m4a` to `Book.m4b` preserves the bytes, so the stat pair matches
/// exactly — but `book_files.format` drives path resolution, and the move
/// writer rewrites path columns only. The classifier must therefore route
/// it Removed + New, where the Phase-B re-parse writes the *correct*
/// format and #1534's title+author relocation still preserves the uuid.
/// Asserting through `book_file_path` is the point: a stale format leaves a
/// book that resolves to a file which is not on disk.
#[tokio::test]
async fn retyping_an_audiobook_extension_reindexes_with_the_new_format_and_keeps_its_uuid() {
    let _covers = CoversTempDir::new("ab_retype_extension");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut seed = indexed_audiobook("Author/Book.m4a", "Book", Some("Seed Author"));
    seed.format = "M4A".into();
    seed.parts[0].filename = "Author/Book.m4a".into();
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![seed],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (book_id, uuid_before): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Author/Book.m4a'")
            .fetch_one(&pool)
            .await
            .unwrap();

    // The same group, same bytes, now carrying the `.m4b` extension.
    let disk = [crate::ebook::StatEntry {
        filename: "Author/Book.m4b".into(),
        scan_key: "Author/Book.m4b".into(),
        mtime_epoch: 100,
        size_bytes: 1000,
        error: None,
    }];
    let db_rows = crate::books::list_indexed_rows_for_formats(
        &pool,
        "/lib",
        crate::audiobook::AUDIOBOOK_FORMATS,
    )
    .await
    .unwrap();
    let diff = crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
    assert!(diff.moved.is_empty(), "a format change is not a relocation");
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.new.len(), 1);

    let mut retyped = indexed_audiobook("Author/Book.m4b", "Book", Some("Seed Author"));
    retyped.format = "M4B".into();
    retyped.parts[0].filename = "Author/Book.m4b".into();
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![retyped],
            moved: diff.moved,
            removed_uuids: diff.removed,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no duplicate book"
    );
    let (uuid_after, scan_key, format): (String, String, String) = sqlx::query_as(
        "SELECT b.uuid, b.scan_key, bf.format FROM books b
           JOIN book_files bf ON bf.book_id = b.id WHERE b.id = ?",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(uuid_after, uuid_before, "identity survives the retype");
    assert_eq!(scan_key, "Author/Book.m4b");
    assert_eq!(format, "M4B", "the re-parse wrote the new format");
    assert_eq!(
        crate::books::book_file_path(&pool, book_id, "M4B")
            .await
            .unwrap(),
        Some(std::path::PathBuf::from("/lib/Author/Book.m4b")),
        "the book resolves to the file that is actually on disk"
    );
    assert_eq!(
        crate::books::book_file_path(&pool, book_id, "M4A")
            .await
            .unwrap(),
        None,
        "no row is left claiming the old extension"
    );
}
