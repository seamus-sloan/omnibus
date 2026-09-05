//! What a purge respects and removes: the file, attachment and override
//! guards, the override row and cover file, the orphaned author, the
//! book's Kobo annotation sync state, the DB-failure path, and the
//! `book_uuid`-first index every user-data probe seeks.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use super::{backdate_missing_since, book_exists, seed_and_make_missing};
use crate::auth::create_user;
use crate::covers::find_override_cover_file;
use crate::metadata_overrides::{
    get_metadata_overrides, upsert_metadata_overrides, write_override_cover,
};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, uuid_by_scan_key, CoversTempDir};

#[tokio::test]
async fn gc_keeps_book_with_files() {
    let _covers = CoversTempDir::new("gc_has_file");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // A live book (has a book_files row) is not missing, so the GC ignores it
    // even if its flag were somehow stale.
    replace_books(
        &pool,
        "/lib",
        vec![indexed("live.epub", Some("Live"), &["A"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(&pool, "live.epub").await;
    // Force a stale flag to prove the NOT EXISTS book_files guard wins.
    sqlx::query("UPDATE books SET is_missing_files = 1, missing_files_since = unixepoch('now','-99 days') WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(book_exists(&pool, &uuid).await, "a book with files is kept");
}

#[tokio::test]
async fn gc_keeps_book_with_merged_uuid_attachment() {
    let _covers = CoversTempDir::new("gc_attachment");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "anchor.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('attached-uuid', ?, 'EPUB', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book still anchoring a cross-format attachment is kept"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_missing_files_override_set() {
    let _covers = CoversTempDir::new("gc_override_exempt");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "wishlist.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    // The manual escape hatch: nothing in production sets the override yet —
    // wishlist/shelf books are protected by their own guards, not this flag.
    sqlx::query("UPDATE books SET is_missing_files_override = 1 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "an intentionally-fileless book is never purged"
    );
}

#[tokio::test]
async fn gc_deletes_override_row_and_cover_file_for_purged_book() {
    let _covers = CoversTempDir::new("gc_override_cover");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "edited.epub").await;
    let ov = MetadataOverrides {
        title: Some("Hand Edited".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, true, user_id)
        .await
        .unwrap();
    write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
    backdate_missing_since(&pool, &uuid, 40).await;

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(!book_exists(&pool, &uuid).await);
    assert!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "override row removed with the purged book"
    );
    assert!(
        find_override_cover_file(&uuid).is_none(),
        "override cover file unlinked"
    );
}

#[tokio::test]
async fn gc_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap_err();
    assert!(matches!(err, MissingFilesError::Db(_)));
}

#[tokio::test]
async fn gc_purges_orphan_author_when_its_last_book_is_deleted() {
    let _covers = CoversTempDir::new("gc_orphan_tax");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "solo.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    // The author survives while the (fileless) book still references it.
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, 1);

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "the purged book's now-bookless author is GC'd");
}

/// The GC's `NOT EXISTS ... WHERE book_uuid = b.uuid` subqueries need a
/// `book_uuid`-first index on each table — pre-existing composites like
/// `(user_id, book_uuid)` or the `(shelf_id, book_uuid)` primary key can't
/// serve the probe because SQLite skips a multi-column index when the leading
/// column isn't in the predicate. Assert every table's plan uses its
/// book-uuid covering index (a `SEARCH` — a `SCAN` would mean the migration
/// was lost or the index name drifted). Same shape as the GC's per-table
/// subquery.
#[tokio::test]
async fn user_data_book_uuid_probes_use_the_book_uuid_first_index() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let cases = [
        ("reading_progress", "idx_reading_progress_book_uuid"),
        ("bookmarks", "idx_bookmarks_book_uuid"),
        ("reading_sessions", "idx_reading_sessions_book_uuid"),
        (
            "reading_progress_daily",
            "idx_reading_progress_daily_book_uuid",
        ),
        (
            "reading_progress_marks",
            "idx_reading_progress_marks_book_uuid",
        ),
        ("listening_sessions", "idx_listening_sessions_book_uuid"),
        ("annotations", "idx_annotations_book_uuid"),
        ("wishlist_entries", "idx_wishlist_book"),
        ("shelf_books", "idx_shelf_books_book_uuid"),
    ];
    for (table, index) in cases {
        let sql = format!("EXPLAIN QUERY PLAN SELECT 1 FROM {table} WHERE book_uuid = ?");
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&sql)
            .bind("any-uuid")
            .fetch_all(&pool)
            .await
            .unwrap();
        let text = plan
            .iter()
            .map(|(_, _, _, s)| s.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            text.contains(&format!("SEARCH {table}")) && text.contains(index),
            "expected {table} probe to SEARCH via {index}, got: {text}"
        );
    }
}

#[tokio::test]
async fn gc_deletes_the_purged_books_kobo_annotation_sync_state() {
    // A per-device annotation watermark (#1278) is bookkeeping, not user
    // data — it must not guard a victim, and it must not survive the purge:
    // an orphaned row would make Reading Services `checkforchanges`
    // re-report the dead uuid forever (its GET can only 404, never ack).
    let _covers = CoversTempDir::new("gc_kobo_sync");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "gone-kobo.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;

    let user_id = create_user(&pool, "kobo-reader", "securepassword1")
        .await
        .unwrap()
        .id;
    let device = crate::kobo_devices::create_device(&pool, user_id, "Kobo")
        .await
        .unwrap();
    crate::kobo::annotations::mark_adopted(&pool, device.id, &uuid)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1, "sync state alone must not guard a victim");

    let sync_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kobo_annotations_sync WHERE book_uuid = ?")
            .bind(&uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        sync_rows, 0,
        "orphaned watermark rows must purge with the book"
    );
}
