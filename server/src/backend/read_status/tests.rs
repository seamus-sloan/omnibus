//! Tests for read/unread-state REST handlers.
use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{ReadStatus, ReadStatusRecord};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn put_status_req(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/api/read-status")
        .method("PUT")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn api_put_read_status_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/read-status")
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "book_uuid": "x", "status": "reading" }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_put_read_status_rejects_blank_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(put_status_req(
            &token,
            serde_json::json!({ "book_uuid": "  ", "status": "reading" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_read_status_returns_404_for_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(put_status_req(
            &token,
            serde_json::json!({ "book_uuid": "no-such-book", "status": "finished" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_read_status_round_trip_last_write_wins() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for status in ["reading", "finished"] {
        let res = app
            .clone()
            .oneshot(put_status_req(
                &token,
                serde_json::json!({ "book_uuid": uuid, "status": status }),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let res = app
        .oneshot(get_with_bearer(&format!("/api/read-status/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<ReadStatusRecord> = serde_json::from_slice(&bytes).unwrap();
    let rec = rec.unwrap();
    assert_eq!(rec.status, ReadStatus::Finished);
    assert!(rec.finished_at.is_some());
}

#[tokio::test]
async fn api_get_read_status_returns_null_when_unset() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(&format!("/api/read-status/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<ReadStatusRecord> = serde_json::from_slice(&bytes).unwrap();
    assert!(rec.is_none());
}

#[tokio::test]
async fn api_read_status_is_isolated_per_user() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    app.clone()
        .oneshot(put_status_req(
            &alice_token,
            serde_json::json!({ "book_uuid": uuid, "status": "finished" }),
        ))
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/read-status/{uuid}"),
            &bob_token,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let rec: Option<ReadStatusRecord> = serde_json::from_slice(&bytes).unwrap();
    assert!(rec.is_none(), "bob sees no read state from alice");
}

#[tokio::test]
async fn api_get_read_status_returns_500_on_db_failure() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, alice.id).await;
    sqlx::query("DROP TABLE book_read_status")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(&format!("/api/read-status/{uuid}"), &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
