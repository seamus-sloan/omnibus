//! Tests for the hidden-formats account preference: set/normalize/validate
//! via `POST /api/account/hidden-formats`. The `/api/auth/me` round trip is
//! covered in `crate::auth::handlers::tests` (the route lives on that router).

use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use omnibus_db::{self as db};

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Build an authenticated JSON POST request.
fn post_json(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn post_hidden_formats_saves_normalized_list_for_the_caller() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(post_json(
            "/api/account/hidden-formats",
            &token,
            serde_json::json!({ "formats": ["CBZ", " m4b ", "cbz"] }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stored = db::auth::get_hidden_formats(&pool, user.id).await.unwrap();
    assert_eq!(stored, vec!["cbz".to_string(), "m4b".to_string()]);
}

#[tokio::test]
async fn post_hidden_formats_rejects_malformed_token_with_422() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(post_json(
            "/api/account/hidden-formats",
            &token,
            serde_json::json!({ "formats": ["not a format!"] }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn post_hidden_formats_without_session_returns_401() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/account/hidden-formats")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"formats":["cbz"]}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
