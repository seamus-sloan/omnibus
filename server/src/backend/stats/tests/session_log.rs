//! `GET /api/stats/sessions`: auth, the malformed cursor, the empty page,
//! stitched sittings newest first, paging through the `before` cursor,
//! per-book scoping, per-user isolation, and the DB-failure path.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use omnibus_shared::{SessionFormat, SessionLogPage};
use tower::ServiceExt;

use super::{now_secs, seed_reading_session};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// The session log is uncached (unlike the aggregate above), so these tests
/// share `now_secs()` freely without colliding on a cache key.
async fn seed_listening_session(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened)
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

#[tokio::test]
async fn api_get_session_log_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_session_log_rejects_a_malformed_before_cursor() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            "/api/stats/sessions?before=nonsense",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_session_log_returns_an_empty_page_for_a_new_user() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn api_get_session_log_returns_stitched_sittings_newest_first() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 100_000;
    // One continuous sit flushed as ten heartbeat rows, then a later listen.
    for i in 0..10 {
        seed_reading_session(&pool, user.id, &uuid, t0 + i * 60, 60).await;
    }
    seed_listening_session(&pool, user.id, &uuid, t0 + 50_000, 1_800).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].format, SessionFormat::Listening);
    assert_eq!(page.entries[0].seconds, 1_800);
    assert_eq!(page.entries[1].format, SessionFormat::Reading);
    assert_eq!(page.entries[1].seconds, 600);
    assert_eq!(page.entries[1].title, "Book A");
}

#[tokio::test]
async fn api_get_session_log_pages_through_the_before_cursor() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 500_000;
    for i in 0..3 {
        seed_reading_session(&pool, user.id, &uuid, t0 + i * 100_000, 600 + i).await;
    }

    let res = app
        .clone()
        .oneshot(get_with_bearer("/api/stats/sessions?limit=2", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let first: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(first.entries.len(), 2);
    let cursor = first.next_before.clone().unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/stats/sessions?limit=2&before={cursor}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let second: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].seconds, 600);
    assert_eq!(second.next_before, None);
}

#[tokio::test]
async fn api_get_session_log_scopes_to_the_requested_book() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid_a) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let (_, uuid_b) = seed_book_with_uuid(&pool, "/lib", "Book B").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 100_000;
    seed_reading_session(&pool, user.id, &uuid_a, t0, 600).await;
    seed_reading_session(&pool, user.id, &uuid_b, t0 + 50_000, 900).await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/stats/sessions?book={uuid_a}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].book_uuid, uuid_a);
}

/// AC7: the log is keyed on the token's user, never on a parameter — so one
/// reader's token can't surface another's sittings on any of the surfaces.
#[tokio::test]
async fn api_get_session_log_never_serves_another_users_sessions() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;
    seed_reading_session(&pool, alice.id, &uuid, now_secs() - 10_000, 600).await;

    for uri in [
        "/api/stats/sessions".to_string(),
        format!("/api/stats/sessions?book={uuid}"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &bob_token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
        assert!(page.entries.is_empty(), "{uri} leaked alice's sessions");
    }
}

#[tokio::test]
async fn api_get_session_log_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

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
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
