//! Direct unit tests for the `sync/audiobooks` write-path helpers, split by
//! bucket into the sibling modules below; the shared scan-root and
//! audiobook seeding fixtures live here. Mirrors `sync/books/tests`: each
//! bucket helper and row writer is exercised against an in-memory DB — the
//! composed `sync_audiobooks` happy path is covered in `sync/tests`.

mod moved;
mod new_changed;
mod removed;
mod writers;

use sqlx::SqlitePool;

use super::shared::insert_new_audiobook;
use crate::test_support::indexed_audiobook;

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
