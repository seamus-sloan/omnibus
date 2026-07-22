//! Tests for the summary-fetch REST handlers.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn post_fetch(uri: &str, token: &str, source: &str) -> Request<Body> {
    let body = serde_json::json!({ "source": source });
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn api_post_summary_fetch_requires_auth() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let body = serde_json::json!({ "source": "OpenLibrary" });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/{uuid}/summary/fetch"))
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_summary_fetch_requires_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post_fetch(
            &format!("/api/ebooks/{uuid}/summary/fetch"),
            &token,
            "OpenLibrary",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_summary_fetch_returns_null_when_hardcover_key_is_unset() {
    // With no Hardcover key configured, `db::fetch_summary` short-circuits to
    // `Ok(None)` before making any network call — safe to exercise here
    // without a wiremock server (that path is covered at the db layer).
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post_fetch(
            &format!("/api/ebooks/{uuid}/summary/fetch"),
            &token,
            "Hardcover",
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "null");
}

#[tokio::test]
async fn api_hardcover_configured_reflects_saved_key_for_any_authenticated_user() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/summary/hardcover-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "false");

    omnibus_db::set_hardcover_api_key(&pool, Some("hc_test_key_1234567890"))
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/summary/hardcover-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "true");
}

#[tokio::test]
async fn api_hardcover_configured_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/summary/hardcover-configured")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
