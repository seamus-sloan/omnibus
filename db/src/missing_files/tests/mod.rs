//! Tests for the missing-files GC, split by sub-topic into the sibling
//! modules below; the seed-then-ghost, backdate and existence fixtures
//! they share live here.

mod lifecycle;
mod purge;
mod retention;

use sqlx::SqlitePool;

use crate::sync::{replace_books, sync_books, SyncPlan};
use crate::test_support::{indexed, uuid_by_scan_key};

/// Seed one book under `/lib`, then make it missing via the Removed bucket —
/// the real path that stamps `is_missing_files` / `missing_files_since`.
async fn seed_and_make_missing(pool: &SqlitePool, file: &str) -> String {
    replace_books(
        pool,
        "/lib",
        vec![indexed(file, Some("T"), &["A"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(pool, file).await;
    sync_books(
        pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    uuid
}

/// Backdate a missing row's clock so it falls outside the retention window.
async fn backdate_missing_since(pool: &SqlitePool, uuid: &str, days_ago: i64) {
    let sql = format!(
        "UPDATE books SET missing_files_since = unixepoch('now', '-{days_ago} days') WHERE uuid = ?"
    );
    sqlx::query(&sql).bind(uuid).execute(pool).await.unwrap();
}

async fn book_exists(pool: &SqlitePool, uuid: &str) -> bool {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    n == 1
}
