//! The Google Books key routes: admin-only get and set, the 422 for an
//! over-long key, the masked read-back that never returns the raw key,
//! and the `configured` flag any authenticated user can read (with its
//! auth and DB-failure paths).

use axum::{
    body::Body,
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db::{auth::SessionKind, test_support::EnvVarGuard};
use tower::ServiceExt;

use super::super::get_google_books_configured;
use super::{body_string, post};
use crate::auth::test_support as auth_test_support;
use crate::auth::AuthUser;
use crate::backend::test_support::*;

/// A minimal `AuthUser` for driving the handler directly (bypassing the
/// `AuthUser` extractor), so a closed pool exercises the handler's own
/// `Err(...) => internal(...)` branch rather than session extraction.
fn fake_user(id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "reader".to_string(),
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        book_detail_scroll_stops: false,
        session_id: 1,
        session_kind: SessionKind::Bearer,
    }
}

#[tokio::test]
async fn api_google_books_key_get_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/google-books-key", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_google_books_key_post_requires_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": "AIzaSecret" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_google_books_key_post_returns_422_when_key_exceeds_max_len() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "a".repeat(omnibus_shared::GOOGLE_BOOKS_API_KEY_MAX_LEN + 1);
    let res = app
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": over_limit }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let body = body_string(res).await;
    assert!(
        body.contains(&omnibus_shared::GOOGLE_BOOKS_API_KEY_MAX_LEN.to_string()),
        "error body should name the limit: {body}"
    );

    // 422 must short-circuit before the KV write.
    assert_eq!(
        omnibus_db::get_google_books_api_key(&pool).await.unwrap(),
        None
    );
}

#[tokio::test]
async fn api_google_books_key_set_then_get_returns_masked_never_raw() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let raw = "AIzaSySupersecretKeyValue1234";
    let set_res = app
        .clone()
        .oneshot(post(
            "/api/google-books-key",
            &token,
            serde_json::json!({ "key": raw }),
        ))
        .await
        .unwrap();
    assert_eq!(set_res.status(), StatusCode::OK);
    let set_body = body_string(set_res).await;
    // The raw key must never be echoed back to the client.
    assert!(!set_body.contains(raw));
    assert!(set_body.contains("\"configured\":true"));
    assert!(set_body.contains("\"source\":\"settings\""));

    let get_res = app
        .oneshot(get_with_bearer("/api/google-books-key", &token))
        .await
        .unwrap();
    assert_eq!(get_res.status(), StatusCode::OK);
    let get_body = body_string(get_res).await;
    assert!(!get_body.contains(raw));
    assert!(get_body.contains("\"configured\":true"));
    // Masked preview keeps the first/last 4 chars around an ellipsis.
    assert!(get_body.contains("AIza\u{2026}1234"));
}

#[tokio::test]
async fn api_google_books_configured_reflects_saved_key_for_any_authenticated_user() {
    // `google_books_key_status` falls back to `GOOGLE_BOOKS_API_KEY`, so the
    // pre-save "false" assertion below only holds with the var removed.
    let _env = EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "false");

    omnibus_db::set_google_books_api_key(&pool, Some("AIzaSySupersecretKeyValue1234"))
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "true");
}

#[tokio::test]
async fn api_google_books_configured_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/scan/google-books-configured")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_google_books_configured_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = get_google_books_configured(fake_user(1), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
