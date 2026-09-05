//! The Removed bucket: `sync_audiobooks_removed` ghosts a group (keeps the
//! `books` row, drops `book_files`), leaves a surviving cross-format
//! attachment unflagged, and drops an attached file recorded only in the
//! `merged_uuids` ledger.

use super::super::shared::attach_audiobook_file;
use super::super::sync_audiobooks_removed;
use super::{book_files_count, seed_audiobook_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed_audiobook, seed_synced_ebook, CoversTempDir};

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
