//! Tests for the API-token REST surface and the extractor accepting `omni_…`
//! bearers: create/list/revoke round trip, per-user scoping, secret-only-once
//! at creation, revocation taking effect immediately, and permission flags
//! carried from the owning account (admin-gated route 403s a reader's token).

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    routing::get,
    Extension, Router,
};
use omnibus_db as db;
use omnibus_shared::{ApiTokenView, CreateApiTokenResponse};
use serde_json::json;
use tower::ServiceExt;

use crate::auth::handlers::auth_router;
use crate::auth::test_support::{bearer_token, create_admin, create_user};
use crate::auth::AdminUser;
use crate::backend::AppState;

async fn app() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let router = auth_router(AppState::new(pool.clone()));
    (router, pool)
}

fn get_req(uri: &str, bearer: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap()
}

fn post_req(uri: &str, bearer: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

fn delete_req(uri: &str, bearer: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("DELETE")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap()
}

async fn body_json<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn create_list_revoke_round_trip() {
    let (app, pool) = app().await;
    let user = create_user(&pool, "alice").await;
    let session = bearer_token(&pool, user.id).await;

    // Create: response carries the omni_-prefixed secret exactly once.
    let res = app
        .clone()
        .oneshot(post_req(
            "/api/auth/api-tokens",
            &session,
            json!({"name": "mcp laptop"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let created: CreateApiTokenResponse = body_json(res).await;
    assert!(created.secret.starts_with("omni_"));
    assert_eq!(created.token.name, "mcp laptop");

    // List: shows the token's name and timestamps, never the secret.
    let res = app
        .clone()
        .oneshot(get_req("/api/auth/api-tokens", &session))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let listed: Vec<ApiTokenView> = body_json(res).await;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.token.id);

    // Revoke, then the listing is empty.
    let res = app
        .clone()
        .oneshot(delete_req(
            &format!("/api/auth/api-tokens/{}", created.token.id),
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app
        .oneshot(get_req("/api/auth/api-tokens", &session))
        .await
        .unwrap();
    let listed: Vec<ApiTokenView> = body_json(res).await;
    assert!(listed.is_empty());
}

#[tokio::test]
async fn api_token_routes_reject_anonymous_requests() {
    let (app, _pool) = app().await;
    for req in [
        Request::builder()
            .uri("/api/auth/api-tokens")
            .body(Body::empty())
            .unwrap(),
        Request::builder()
            .uri("/api/auth/api-tokens")
            .method("POST")
            .header("content-type", "application/json")
            .body(Body::from(json!({"name": "x"}).to_string()))
            .unwrap(),
        Request::builder()
            .uri("/api/auth/api-tokens/1")
            .method("DELETE")
            .body(Body::empty())
            .unwrap(),
    ] {
        let res = app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}

#[tokio::test]
async fn create_rejects_blank_name_with_400() {
    let (app, pool) = app().await;
    let user = create_user(&pool, "alice").await;
    let session = bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(post_req(
            "/api/auth/api-tokens",
            &session,
            json!({"name": "  "}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn revoke_of_another_users_token_is_404() {
    let (app, pool) = app().await;
    let alice = create_user(&pool, "alice").await;
    let bob = create_user(&pool, "bob").await;
    let bobs = db::auth::create_api_token(&pool, bob.id, "bobs")
        .await
        .unwrap();
    let session = bearer_token(&pool, alice.id).await;
    let res = app
        .oneshot(delete_req(
            &format!("/api/auth/api-tokens/{}", bobs.token.id),
            &session,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_token_authenticates_requests_until_revoked() {
    // AC1 + AC3, end to end through the extractor: the omni_ bearer works,
    // then stops working the moment it is revoked.
    let (app, pool) = app().await;
    let user = create_user(&pool, "alice").await;
    let minted = db::auth::create_api_token(&pool, user.id, "mcp")
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(get_req("/api/auth/me", &minted.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    db::auth::revoke_api_token_for_user(&pool, user.id, minted.token.id)
        .await
        .unwrap();
    let res = app
        .oneshot(get_req("/api/auth/me", &minted.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// Minimal admin-gated route, for asserting a token carries exactly its
/// owning account's flags (AC4) without dragging the whole REST router in.
fn admin_probe(pool: sqlx::SqlitePool) -> Router {
    async fn probe(_admin: AdminUser) -> StatusCode {
        StatusCode::OK
    }
    Router::new()
        .route("/probe", get(probe))
        .layer(Extension(pool))
}

#[tokio::test]
async fn api_token_carries_owning_accounts_permission_flags() {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let reader = create_user(&pool, "reader").await;
    let admin = create_admin(&pool, "boss").await;
    let reader_token = db::auth::create_api_token(&pool, reader.id, "r")
        .await
        .unwrap();
    let admin_token = db::auth::create_api_token(&pool, admin.id, "a")
        .await
        .unwrap();

    let app = admin_probe(pool.clone());
    let res = app
        .clone()
        .oneshot(get_req("/probe", &reader_token.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);

    let res = app
        .oneshot(get_req("/probe", &admin_token.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
