//! `PUT /api/stats/goal` and `/goal/daily`: auth, saving and the next
//! summary reflecting it, a null target clearing one goal or one daily
//! kind, the kind / target / year rejections and per-kind maxima, an
//! unusable offset accepted on both routes, and the DB-failure paths.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{DailyGoals, ReadingGoal, StatsSummary};
use tower::ServiceExt;

use super::now_secs;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Shared by both goal routes, which differ only in their path — and, for the
/// offset tests, in the query string on it.
fn put_json_req(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("PUT")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn put_goal_req(token: &str, body: serde_json::Value) -> Request<Body> {
    put_json_req("/api/stats/goal", token, body)
}

async fn goal_body(res: axum::response::Response) -> Option<ReadingGoal> {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_put_stats_goal_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats/goal")
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "target": 24 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// The happy path plus AC5: the summary read straight after the write already
/// carries the new target rather than the pre-save cached one. Uses
/// `range=week` — a range no other test in this binary caches.
#[tokio::test]
async fn api_put_stats_goal_saves_and_the_next_summary_read_reflects_it() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user.id)
    .bind(&uuid)
    .bind(now_secs())
    .bind(now_secs())
    .execute(&pool)
    .await
    .unwrap();

    // Warm the cache with the goal-less answer first, so a stale read would
    // be visible below.
    let warm = app
        .clone()
        .oneshot(get_with_bearer("/api/stats?range=week", &token))
        .await
        .unwrap();
    assert_eq!(warm.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 24 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let goal = goal_body(res).await.expect("a set target returns a goal");
    assert_eq!(goal.target, 24);
    assert_eq!(goal.current, 1);

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=week", &token))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.goal.map(|g| g.target), Some(24));
}

#[tokio::test]
async fn api_put_stats_goal_with_a_null_target_clears_the_goal() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    app.clone()
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 12 })))
        .await
        .unwrap();
    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": null })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(goal_body(res).await.is_none());
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_out_of_range_target() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 0 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_unsupported_kind() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 500 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_out_of_range_year() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(
            &token,
            serde_json::json!({ "year": 1234, "target": 12 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE reading_goals")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 24 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

fn put_daily_goal_req(token: &str, body: serde_json::Value) -> Request<Body> {
    put_json_req("/api/stats/goal/daily", token, body)
}

async fn daily_body(res: axum::response::Response) -> DailyGoals {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_put_stats_daily_goal_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats/goal/daily")
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "kind": "pages", "target": 30 }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// The happy path: one write answers with both kinds, so the client can redraw
/// the whole band without a follow-up read.
#[tokio::test]
async fn api_put_stats_daily_goal_saves_and_answers_with_both_kinds() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 30 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let goals = daily_body(res).await;
    assert_eq!(goals.pages.as_ref().map(|g| g.target), Some(30));
    assert_eq!(goals.pages.as_ref().map(|g| g.current), Some(0));
    assert!(goals.minutes.is_none());

    let res = app
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "minutes", "target": 20 }),
        ))
        .await
        .unwrap();
    let goals = daily_body(res).await;
    assert_eq!(
        goals.pages.map(|g| g.target),
        Some(30),
        "untouched by the second write"
    );
    assert_eq!(goals.minutes.map(|g| g.target), Some(20));
}

/// AC5: an absent target clears that kind and leaves the other standing.
#[tokio::test]
async fn api_put_stats_daily_goal_with_a_null_target_clears_only_that_kind() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for body in [
        serde_json::json!({ "kind": "pages", "target": 30 }),
        serde_json::json!({ "kind": "minutes", "target": 20 }),
    ] {
        app.clone()
            .oneshot(put_daily_goal_req(&token, body))
            .await
            .unwrap();
    }

    let res = app
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": null }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let goals = daily_body(res).await;
    assert!(goals.pages.is_none());
    assert_eq!(goals.minutes.map(|g| g.target), Some(20));
}

#[tokio::test]
async fn api_put_stats_daily_goal_rejects_a_kind_with_no_daily_measurement() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "books", "target": 1 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// AC2, and the part a shared bound would get wrong: 1,500 is a legal day of
/// pages and an impossible day of minutes, so the same target has to be
/// accepted on one kind and refused on the other.
#[tokio::test]
async fn api_put_stats_daily_goal_bounds_each_kind_against_its_own_maximum() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 1500 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "minutes", "target": 1500 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let res = app
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 0 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_daily_goal_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE reading_goals")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(put_daily_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 30 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Both goal routes read the offset on the same lenient terms `GET /api/stats`
/// does. It only decides whose *today* the returned progress is measured on, so
/// an unreadable one falls back — refusing the write would lose the reader their
/// goal over a hint.
#[tokio::test]
async fn api_put_stats_goals_accept_an_unusable_offset_on_both_routes() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for raw in ["", "abc", "99999"] {
        let res = app
            .clone()
            .oneshot(put_json_req(
                &format!("/api/stats/goal?utc_offset_minutes={raw}"),
                &token,
                serde_json::json!({ "target": 24 }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "utc_offset_minutes={raw:?}");

        let res = app
            .clone()
            .oneshot(put_json_req(
                &format!("/api/stats/goal/daily?utc_offset_minutes={raw}"),
                &token,
                serde_json::json!({ "kind": "pages", "target": 30 }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "utc_offset_minutes={raw:?}");
    }
}
