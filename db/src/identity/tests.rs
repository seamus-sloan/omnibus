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
async fn backfill_fills_multiple_books_and_a_merged_attachment_in_one_pass() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    // Two native books with NULL scan_keys …
    for (uuid, path, title) in [("bk-1", "A1", "T1"), ("bk-2", "A2", "T2")] {
        sqlx::query(
            "INSERT INTO books (uuid, scan_key, library_id, path, title) VALUES (?, NULL, 1, ?, ?)",
        )
        .bind(uuid)
        .bind(path)
        .bind(title)
        .execute(&pool)
        .await
        .unwrap();
    }
    let bk1_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'bk-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES ((SELECT id FROM books WHERE uuid='bk-1'), 'EPUB', 'T1', 10, 100),
                ((SELECT id FROM books WHERE uuid='bk-2'), 'EPUB', 'T2', 10, 100),
                ((SELECT id FROM books WHERE uuid='bk-1'), 'M4B',  'T1', 20, 200)",
    )
    .execute(&pool)
    .await
    .unwrap();
    // … and an attachment row (the M4B on bk-1) with a NULL scan_key.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('att-1', ?, 'M4B', '/lib', NULL)",
    )
    .bind(bk1_id)
    .execute(&pool)
    .await
    .unwrap();

    backfill_scan_keys(&pool).await.unwrap();

    // Both books get their *native* (non-attached) format's key; bk-1's native
    // format is EPUB (M4B is the attachment), so it must resolve to the EPUB key.
    let keys: Vec<(String, Option<String>)> =
        sqlx::query_as("SELECT uuid, scan_key FROM books ORDER BY uuid")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(
        keys,
        vec![
            ("bk-1".to_string(), Some("A1/T1.epub".to_string())),
            ("bk-2".to_string(), Some("A2/T2.epub".to_string())),
        ]
    );
    // The merged key reconstructs from the attached file's *own* `book_files`
    // location override (`COALESCE(bf.path, '')`), which is NULL here, so the
    // key is the bare leaf — no parent path.
    let att: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM merged_uuids WHERE uuid = 'att-1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(att.as_deref(), Some("T1.m4b"));
}

#[tokio::test]
async fn backfill_scan_keys_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = backfill_scan_keys(&pool).await.unwrap_err();
    assert!(matches!(err, IdentityError::Db(_)));
}
