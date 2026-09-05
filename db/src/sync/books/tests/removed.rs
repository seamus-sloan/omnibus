//! The Removed bucket: `sync_removed` ghosts a book (keeps the `books`
//! row, drops `book_files`), leaves a surviving cross-format attachment
//! unflagged, clears `has_cover` for the backfill, and
//! `wipe_per_book_link_rows` empties every link table.

use super::super::{sync_books, sync_removed, wipe_per_book_link_rows, SyncPlan};
use super::{book_files_count, seed_book_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::test_support::{count_rows, CoversTempDir};

/// `sync_removed` ghosts a book (F2): the file is gone, so its
/// `book_files` row is dropped, but the durable `books` row survives so
/// the uuid — and any user data keyed on it — persists.
#[tokio::test]
async fn sync_removed_retains_books_row_and_removes_book_files_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "file present pre-ghost"
    );

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[uuid]).await.unwrap();
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
    // The retained row is flagged missing so F10 GC can later reap it.
    let flagged: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flagged, 1, "retained row is flagged missing");
}

/// A book whose own ebook file is removed, but which still holds a
/// cross-format attachment (a different format's file, recorded in
/// `merged_uuids` and still present), is not flagged missing — the surviving
/// file means the book isn't actually fileless.
#[tokio::test]
async fn sync_removed_does_not_flag_missing_when_a_cross_format_attachment_survives() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Forge a surviving cross-format attachment: a book_files row in a
    // different format, backed by its own merged_uuids ledger entry.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'M4B', 'a', 1000, 100, 'a.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('attached-m4b', ?, 'M4B', '/audio', 'a.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[uuid]).await.unwrap();
    tx.commit().await.unwrap();

    // The EPUB row is dropped, the M4B survives, and the book must not be
    // flagged missing since it still holds a file.
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        0,
        "the book's own native file row is dropped"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
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

/// One `sync_removed` call ghosts every uuid in the batch — proving the
/// batched DELETE + UPDATE covers N books in a single invocation.
#[tokio::test]
async fn sync_removed_ghosts_multiple_books_in_one_call() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut uuids = Vec::new();
    for i in 0..3 {
        let book_id = seed_book_with_file(&pool, library_id, &format!("b{i}.epub")).await;
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
    sync_removed(&mut tx, library_id, &uuids).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        3,
        "all three books rows retained"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0,
        "all three book_files rows dropped in one call"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books WHERE is_missing_files = 1"
        )
        .await,
        3,
        "all three rows flagged missing"
    );
}

/// `sync_removed` is a no-op for an empty batch (early return) and does
/// not touch any rows.
#[tokio::test]
async fn sync_removed_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "keep.epub").await;

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[]).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "empty removed batch leaves the file row intact"
    );
}

/// `wipe_per_book_link_rows` clears all seven per-book tables for the
/// given `book_id` (the format-scoped `book_files` row plus the six link
/// tables) so `sync_changed` can re-insert without hitting UNIQUE
/// constraints.
#[tokio::test]
async fn wipe_per_book_link_rows_clears_all_seven_tables_for_the_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;

    // Pre-condition: every per-book table has a row for this book.
    for (table, col) in [
        ("book_files", "book_id"),
        ("book_identifiers", "book_id"),
        ("books_authors_link", "book"),
        ("books_tags_link", "book"),
        ("books_publishers_link", "book"),
        ("books_series_link", "book"),
        ("books_languages_link", "book"),
    ] {
        let n = count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE {col} = {book_id}"),
        )
        .await;
        assert_eq!(n, 1, "{table} should have a seeded row before the wipe");
    }

    let mut tx = pool.begin().await.unwrap();
    // The seed file is an EPUB, so wipe that format's book_files row.
    wipe_per_book_link_rows(&mut tx, book_id, "EPUB")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    for (table, col) in [
        ("book_files", "book_id"),
        ("book_identifiers", "book_id"),
        ("books_authors_link", "book"),
        ("books_tags_link", "book"),
        ("books_publishers_link", "book"),
        ("books_series_link", "book"),
        ("books_languages_link", "book"),
    ] {
        let n = count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE {col} = {book_id}"),
        )
        .await;
        assert_eq!(n, 0, "{table} should be empty for the book after the wipe");
    }
}

/// The post-commit reconcile unlinks the cover file for every removed uuid, so
/// leaving `has_cover = 1` strands the book with a flag the filesystem no
/// longer backs. Both repair paths key on that flag — `maybe_adopt_cover`
/// returns early on `has_cover != 0` and `backfill_covers` selects on
/// `has_cover = 0` — so the book would never regain a cover (#2321).
#[tokio::test]
async fn removed_book_clears_has_cover_so_the_cover_backfill_can_re_extract() {
    let _covers = CoversTempDir::new("sync_books_removed_has_cover");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (has_cover, is_missing): (i64, i64) =
        sqlx::query_as("SELECT has_cover, is_missing_files FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        has_cover, 0,
        "the flag matches the cover file the reconcile unlinked"
    );
    assert_eq!(is_missing, 1, "the book is still flagged missing");
}
