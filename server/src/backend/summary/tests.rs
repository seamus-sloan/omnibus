//! Tests for the summary-fetch REST handlers.

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db::{auth::SessionKind, test_support::EnvVarGuard};
use tower::ServiceExt;

use super::{get_summary_sources, AppState};
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
        session_id: 1,
        session_kind: SessionKind::Bearer,
    }
}

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
    // without a wiremock server (that path is covered at the db layer). The
    // guard removes an ambient `HARDCOVER_API_KEY` (a developer's `.env`)
    // that would otherwise send this test out to the real Hardcover API.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
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
async fn api_summary_sources_reflects_configured_keys_for_any_authenticated_user() {
    // The plan falls back to the env keys, so the keyless assertion below only
    // holds with both vars removed. Both clear under ONE `ENV_LOCK` acquisition
    // via `also_set` — a second `EnvVarGuard::set` would deadlock re-locking the
    // mutex the first guard still holds.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/summary/sources")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    // No keys → Google Books, then Open Library as the keyless fallback.
    assert_eq!(body_string(res).await, r#"["GoogleBooks","OpenLibrary"]"#);

    omnibus_db::set_hardcover_api_key(&pool, Some("hc_test_key_1234567890"))
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/summary/sources")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    // A key exists → Open Library is dropped.
    assert_eq!(body_string(res).await, r#"["Hardcover","GoogleBooks"]"#);
}

#[tokio::test]
async fn api_summary_sources_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/summary/sources")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_summary_sources_returns_500_when_db_unavailable() {
    let (_app, _state, pool) = fixture().await;
    pool.close().await;
    let res = get_summary_sources(fake_user(1), State(AppState::new(pool))).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
