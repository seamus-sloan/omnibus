//! `is_stale` and its decision helper: the freshness window boundaries, the
//! clock-failure fallback, and the never-indexed case.

use crate::pool::init_db;

use super::super::*;
use super::now_secs;

/// Seed a `scan_roots` row for `path` with an explicit `last_indexed`
/// epoch-seconds value. There's no public writer for `last_indexed`
/// that lets a test set an arbitrary timestamp (`sync_books` always
/// stamps "now"), so the `is_stale` window tests insert the row
/// directly — exactly the columns `last_indexed_at` reads back.
async fn seed_last_indexed(pool: &SqlitePool, path: &str, last_indexed: i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name, last_indexed) VALUES (?, ?, ?)")
        .bind(path)
        .bind(path)
        .bind(last_indexed)
        .execute(pool)
        .await
        .unwrap();
}

#[test]
fn is_stale_decision_respects_window_boundaries() {
    // Pure window logic: not stale strictly inside the window, stale at
    // and past the horizon.
    let last = 1_700_000_000;
    assert!(!is_stale_decision(last, last));
    assert!(!is_stale_decision(last, last + REFRESH_AFTER_SECS - 1));
    assert!(is_stale_decision(last, last + REFRESH_AFTER_SECS));
    assert!(is_stale_decision(last, last + REFRESH_AFTER_SECS + 1));
}

#[test]
fn is_stale_decision_clock_failure_serves_stale() {
    // The clock-failure fallback in `is_stale` substitutes `last` for an
    // unreadable `now`, so the decision is evaluated with `now == last`.
    // Pin the documented consequence: not stale (serve the existing index
    // rather than thrash the disk on every poll).
    let last = 1_700_000_000;
    assert!(!is_stale_decision(last, last));
}

#[tokio::test]
async fn is_stale_returns_true_when_no_index_exists() {
    // Fresh DB: the library has never been indexed, so `last_indexed_at`
    // is None and `is_stale` short-circuits to true (kick off the first
    // index). No `libraries` row at all is the strongest form of this.
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn is_stale_returns_false_within_window() {
    // Indexed 30s ago — well inside REFRESH_AFTER_SECS (1h), so no
    // reindex is due yet.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_last_indexed(&pool, "/lib", now_secs() - 30).await;
    assert!(!is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn is_stale_returns_true_past_window() {
    // Indexed just past the refresh horizon (REFRESH_AFTER_SECS + 1
    // seconds ago) — the index is stale and a reindex is due.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_last_indexed(&pool, "/lib", now_secs() - REFRESH_AFTER_SECS - 1).await;
    assert!(is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn is_stale_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = is_stale(&pool, "/lib").await.unwrap_err();
    assert!(matches!(err, IndexerError::Db(_)));
}
