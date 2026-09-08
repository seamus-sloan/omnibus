//! The New and Changed buckets: `sync_audiobooks_new` inserts rows or
//! rewrites a fileless book in place, and `sync_audiobooks_changed` updates
//! in place, promotes to an insert on a TOCTOU miss, and refreshes an
//! attached file through the `merged_uuids` ledger.

use super::super::shared::attach_audiobook_file;
use super::super::{sync_audiobooks_changed, sync_audiobooks_new, sync_audiobooks_removed};
use super::{book_files_count, seed_audiobook_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed_audiobook, seed_synced_ebook, CoversTempDir};

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
