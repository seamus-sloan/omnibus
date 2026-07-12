//! Tests for the reading-stats REST handler.
//!
//! The `db::stats` cache is process-wide and keyed on `(user_id, range)`,
//! and every fixture pool restarts user ids at 1 — so each content-asserting
//! test here must use a distinct range to keep its cache key unique across
//! the test binary.

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
