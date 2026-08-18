//! `POST /api/metadata/editions/search` — fan-out edition search.
//!
//! The happy path here is deliberately network-free: it names only the
//! unconfigured Hardcover provider, so the handler answers 200 with a
//! `not_configured` report without a provider ever being asked. Provider
//! behaviour itself is covered by the `omnibus-db` fan-out suite.

use axum::{
    body::{to_bytes, Body},
    extract::State,
    http::{header::AUTHORIZATION, Request, StatusCode},
    Json,
};
use omnibus_db::{auth::SessionKind, test_support::EnvVarGuard};
use omnibus_shared::metadata_lookup::EditionSearchRequest;
use tower::ServiceExt;

use super::post_edition_search;
use crate::auth::test_support as auth_test_support;
use crate::auth::AuthUser;
use crate::backend::test_support::*;

const URI: &str = "/api/metadata/editions/search";

/// A minimal edit-permitted `AuthUser` for driving the handler directly, so a
/// closed pool exercises its own `Err(...) => internal(...)` branch rather
/// than session extraction.
fn fake_editor(id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "editor".to_string(),
        is_admin: false,
        can_upload: false,
        can_edit: true,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        session_id: 1,
        session_kind: SessionKind::Bearer,
    }
}

fn post(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(URI)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .expect("request should build")
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    let bytes = to_bytes(res.into_body(), 1024 * 1024)
        .await
        .expect("body should read");
    serde_json::from_slice(&bytes).expect("body should be json")
}

#[tokio::test]
async fn api_post_edition_search_returns_200_with_a_per_source_report() {
    // Pin both keys off so the report doesn't drift with a developer's `.env`.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            &token,
            serde_json::json!({ "query": "effective java", "providers": ["hardcover"] }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let body = body_json(res).await;
    assert_eq!(body["editions"].as_array().map(Vec::len), Some(0));
    let sources = body["sources"].as_array().expect("sources should be array");
    assert_eq!(sources.len(), 1, "only the named provider is reported");
    assert_eq!(sources[0]["provider"], "hardcover");
    assert_eq!(sources[0]["status"], "not_configured");
}

#[tokio::test]
async fn api_post_edition_search_rejects_a_blank_query_with_400() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(&token, serde_json::json!({ "query": "   " })))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_edition_search_rejects_an_oversized_query_with_400() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "boss").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let oversized = "x".repeat(omnibus_shared::metadata_lookup::EDITION_SEARCH_QUERY_MAX_LEN + 1);
    let res = app
        .oneshot(post(&token, serde_json::json!({ "query": oversized })))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_edition_search_rejects_a_user_without_edit_permission_with_403() {
    // A plain user from `create_user` has `can_edit = false`, and the check
    // runs before validation so no provider call is ever reached.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            &token,
            serde_json::json!({ "query": "effective java" }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_edition_search_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(URI)
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "query": "effective java" }).to_string(),
                ))
                .expect("request should build"),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_edition_search_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = post_edition_search(
        fake_editor(1),
        State(state),
        Json(EditionSearchRequest {
            query: "effective java".into(),
            providers: None,
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
