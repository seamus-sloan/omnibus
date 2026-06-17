use super::*;
use crate::pool::init_db;

#[test]
fn reconstruct_scan_key_appends_ext_for_files_and_omits_it_for_mp3_folders() {
    // EPUB / single m4b / single mp3 → file path with lowercased extension.
    assert_eq!(
        reconstruct_scan_key("Author", "Title", "EPUB", 0),
        "Author/Title.epub"
    );
    assert_eq!(
        reconstruct_scan_key("Author", "Book", "M4B", 1),
        "Author/Book.m4b"
    );
    assert_eq!(
        reconstruct_scan_key("Author", "track", "MP3", 1),
        "Author/track.mp3"
    );
    // Multi-part mp3 folder → directory path, no extension.
    assert_eq!(
        reconstruct_scan_key("Author", "Book", "MP3", 12),
        "Author/Book"
    );
    // Empty parent → leaf only.
    assert_eq!(reconstruct_scan_key("", "Title", "EPUB", 0), "Title.epub");
}

#[tokio::test]
async fn backfill_fills_null_scan_keys_once_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    // Simulate a pre-0026 row: a book + its book_files with a NULL scan_key.
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES ('bk-1', NULL, 1, 'Author', 'Title')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (1, 'EPUB', 'Title', 10, 100)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // The row was inserted *after* init_db's backfill, so run it explicitly.
    backfill_scan_keys(&pool).await.unwrap();
    let key: Option<String> = sqlx::query_scalar("SELECT scan_key FROM books WHERE uuid = 'bk-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(key.as_deref(), Some("Author/Title.epub"));

    // Second run is a no-op (the WHERE scan_key IS NULL guard).
    backfill_scan_keys(&pool).await.unwrap();
    let still: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM books WHERE uuid = 'bk-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(still.as_deref(), Some("Author/Title.epub"));
}

#[tokio::test]
async fn backfill_scan_keys_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = backfill_scan_keys(&pool).await.unwrap_err();
    assert!(matches!(err, IdentityError::Db(_)));
}
