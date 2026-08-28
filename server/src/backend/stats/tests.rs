//! Tests for the reading-stats REST handler. The `db::stats` cache is
//! process-wide and keyed on `(user_id, range)`, and every fixture pool
//! restarts user ids at 1 — so each content-asserting test uses a distinct
//! range to keep its cache key unique across the test binary.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use omnibus_shared::{StatsRange, StatsSummary};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

async fn seed_reading_session(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tokio::test]
async fn api_get_stats_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_stats_rejects_unknown_range() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/stats?range=fortnight", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_stats_defaults_to_the_month_range() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_reading_session(&pool, user.id, &uuid, now_secs(), 600).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.range, StatsRange::Month);
    assert_eq!(summary.reading_seconds, 600);
    assert_eq!(summary.sessions, 1);
}

#[tokio::test]
async fn api_get_stats_honors_the_range_param() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // Late-2023 anchor: inside the all-time window, outside week/month/year.
    seed_reading_session(&pool, user.id, &uuid, 1_700_000_000, 900).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=all_time", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.range, StatsRange::AllTime);
    assert_eq!(summary.reading_seconds, 900);
}

/// Mirrors the ebooks/audiobooks sibling suites' DROP TABLE pattern: a DB
/// failure inside `db::stats` must surface as a 500. Uses `range=year` —
/// a range no other test caches — so the process-wide stats cache can't
/// serve a stale 200 here.
#[tokio::test]
async fn api_get_stats_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // FKs off before the DROP so references to the sessions table can't
    // turn the drop itself into an error (same as the ebooks suite).
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=year", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// --- GET /api/library-size ----------------------------------------------

#[tokio::test]
async fn api_get_library_size_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/library-size")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_library_size_returns_totals_with_their_coverage() {
    let (app, _state, pool) = fixture().await;
    let (book_id, _) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    sqlx::query("UPDATE books SET word_count = 275 WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // The aggregate is cached library-wide across the whole test binary, so
    // this test has to clear it rather than pick a unique key the way the
    // per-user ones do.
    omnibus_db::stats::invalidate_library_size();

    let res = app
        .oneshot(get_with_bearer("/api/library-size", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let size: omnibus_shared::LibrarySize = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(size.books, 1);
    assert_eq!(size.words.total, 275);
    assert_eq!(size.words.books, 1, "the total must carry its denominator");
}

#[tokio::test]
async fn api_get_library_size_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::stats::invalidate_library_size();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(get_with_bearer("/api/library-size", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    omnibus_db::stats::invalidate_library_size();
}
