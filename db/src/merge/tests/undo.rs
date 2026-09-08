//! `undo_merge`: the source book and its file come back with the scan
//! key intact, an unparseable snapshot timestamp is stamped now, a deleted
//! file restores a fileless book, and the unknown-log, double-undo and
//! corrupt-snapshot rejections.

use super::super::*;
use super::book_id_by_uuid;
use crate::books::list_indexed_rows_for_formats;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{
    count_rows as count, indexed_audiobook, seed_synced_audiobook as seed_audiobook,
    seed_synced_ebook as seed_ebook,
};

#[tokio::test]
async fn undo_merge_restores_source_book_and_moves_file_back() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    let restored = undo_merge(&pool, out.merge_log_id).await.unwrap();
    assert_eq!(restored, source);

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    let new_source_id = book_id_by_uuid(&pool, &source).await;
    // The M4B file row (and its part) came back to the restored book.
    let owner: i64 = sqlx::query_scalar("SELECT book_id FROM book_files WHERE format = 'M4B'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, new_source_id);
    // Restored metadata and links.
    let (title, path): (String, String) =
        sqlx::query_as("SELECT title, path FROM books WHERE id = ?")
            .bind(new_source_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "Drakula");
    assert_eq!(path, "B");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM books_authors_link").await,
        2
    );
    // Guard cleared; log marked undone; FTS has both rows again.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
    let undone: Option<i64> = sqlx::query_scalar("SELECT undone_at FROM merge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(undone.is_some());
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 2);
}

#[tokio::test]
async fn undo_merge_restores_scan_key_so_reindex_finds_no_duplicate() {
    // Regression: undo used to omit `books.scan_key` from the recreated row,
    // leaving it NULL. The next reindex then couldn't match the on-disk file
    // to the restored (scan_key-less) row and inserted a second copy — the
    // duplicate that destabilized the audiobook E2E suite.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let source_scan_key = crate::helpers::scan_key_for("B/Drakula.m4b");

    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    undo_merge(&pool, out.merge_log_id).await.unwrap();

    // The recreated row carries the source's original scan_key back.
    let restored_scan_key: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM books WHERE uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restored_scan_key.as_deref(), Some(source_scan_key.as_str()));

    // A reindex now matches the on-disk file to the restored native row and
    // classifies it Unchanged — no `new` entry, so no duplicate insert.
    let ab = indexed_audiobook("B/Drakula.m4b", "Drakula", Some("Bram Stoker"));
    let db_rows = list_indexed_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        scan_key: ab.scan_key.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    assert!(
        diff.new.is_empty(),
        "restored book must not reindex as New (would duplicate): {:?}",
        diff.new
    );
    assert_eq!(diff.unchanged, vec![source]);
}

#[tokio::test]
async fn undo_merge_stamps_now_when_snapshot_timestamp_is_unparseable() {
    // A pre-0038 merge snapshot stored `timestamp` as an ISO string, which
    // `de_epoch_flexible` can't convert to an epoch (-> None). The recreate must
    // fall back to now rather than leaving the restored row's date-added NULL.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    // Rewrite the persisted snapshot to the legacy string form.
    let json: String = sqlx::query_scalar("SELECT source_metadata FROM merge_log WHERE id = ?")
        .bind(out.merge_log_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut snap: serde_json::Value = serde_json::from_str(&json).unwrap();
    snap["timestamp"] = serde_json::json!("2024-01-02 03:04:05");
    sqlx::query("UPDATE merge_log SET source_metadata = ? WHERE id = ?")
        .bind(snap.to_string())
        .bind(out.merge_log_id)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    let ts: Option<i64> = sqlx::query_scalar("SELECT timestamp FROM books WHERE uuid = ?")
        .bind(&source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        matches!(ts, Some(e) if e > 1_700_000_000),
        "restored timestamp should fall back to now, got {ts:?}"
    );
}

#[tokio::test]
async fn undo_merge_with_deleted_file_restores_fileless_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    // The moved file vanished from disk and a reindex dropped its row.
    sqlx::query("DELETE FROM book_files WHERE format = 'M4B'")
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    // The restored source exists with zero files — a legal state.
    let new_source_id = book_id_by_uuid(&pool, &source).await;
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(new_source_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(files, 0);
}

#[tokio::test]
async fn undo_merge_rejects_unknown_log_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let err = undo_merge(&pool, 999).await.unwrap_err();
    assert!(matches!(err, MergeError::LogNotFound));
}

#[tokio::test]
async fn undo_merge_rejects_double_undo() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    undo_merge(&pool, out.merge_log_id).await.unwrap();
    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::AlreadyUndone));
}

#[tokio::test]
async fn undo_merge_returns_snapshot_error_when_source_metadata_is_corrupt() {
    // `undo_merge` replays `merge_log.source_metadata` via
    // `serde_json::from_str`; a row whose snapshot JSON is corrupt (truncated
    // by a bad manual edit or a partial write) must surface as
    // `MergeError::Snapshot` — the `#[from] serde_json::Error` variant — not
    // as a panic or a `Db` error. Overwrite a real log entry's snapshot with
    // malformed JSON to drive the decode failure.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    sqlx::query("UPDATE merge_log SET source_metadata = ? WHERE id = ?")
        .bind("{ this is not valid json")
        .bind(out.merge_log_id)
        .execute(&pool)
        .await
        .unwrap();

    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::Snapshot(_)), "got {err:?}");
    assert!(
        err.to_string()
            .starts_with("merge snapshot encode/decode failed"),
        "got {err}"
    );
}
