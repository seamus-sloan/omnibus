//! Tests for `GET`/`POST /api/ebooks/{uuid}/external-ratings`.
//!
//! Deliberately network-free: every POST case here either fails before the
//! provider fan-out (permission, ISBN validation, unknown book, DB failure) or
//! drives the handler directly. The fan-out's own 200 path — one row per
//! provider that reported a score — is covered against wiremock in
//! `omnibus_db::external_ratings`.

use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::{header::AUTHORIZATION, Request, StatusCode},
    Json,
};
use omnibus_db::auth::SessionKind;
use omnibus_shared::external_ratings::{ProviderRating, RefreshRatingsRequest};
use omnibus_shared::metadata_lookup::MetadataProvider;
use tower::ServiceExt;

use super::{get_external_ratings, post_external_ratings};
use crate::auth::test_support as auth_test_support;
use crate::auth::AuthUser;
use crate::backend::test_support::*;

/// Effective Java — a valid ISBN-13, so validation never masks the case under
/// test.
const ISBN13: &str = "9780134685991";

fn path_for(uuid: &str) -> String {
    format!("/api/ebooks/{uuid}/external-ratings")
}

/// A minimal `AuthUser` for driving a handler directly, so a closed pool
/// exercises the handler's own `internal(...)` branch rather than session
/// extraction.
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

fn fake_editor(id: i64) -> AuthUser {
    AuthUser {
        can_edit: true,
        ..fake_user(id)
    }
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

fn post(uri: &str, token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

/// Store one provider's rating directly, standing in for an apply.
async fn store_rating(pool: &sqlx::SqlitePool, uuid: &str, provider: MetadataProvider, score: f64) {
    omnibus_db::external_ratings::upsert_rating(
        pool,
        uuid,
        provider,
        &ProviderRating::new(Some(score), 5.0, Some(12), Some("https://x/y".into())).unwrap(),
    )
    .await
    .expect("seed rating");
}

// ── GET ──────────────────────────────────────────────────────────

#[tokio::test]
async fn api_get_external_ratings_returns_every_source_attributed() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Rated").await;
    store_rating(&pool, &uuid, MetadataProvider::GoogleBooks, 4.5).await;
    store_rating(&pool, &uuid, MetadataProvider::OpenLibrary, 3.75).await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(&path_for(&uuid), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    let rows = body.as_array().expect("ratings should be a JSON array");
    assert_eq!(rows.len(), 2, "both sources should be reported");
    let google = rows
        .iter()
        .find(|r| r["provider"] == "google_books")
        .expect("google books should be listed");
    assert_eq!(google["display_name"], "Google Books");
    assert_eq!(google["rating"], 4.5);
    assert_eq!(google["rating_max"], 5.0);
    assert_eq!(google["ratings_count"], 12);
}

#[tokio::test]
async fn api_get_external_ratings_returns_an_empty_list_for_a_book_with_none() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Unrated").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(&path_for(&uuid), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_string(res).await, "[]");
}

#[tokio::test]
async fn api_get_external_ratings_requires_auth() {
    let (app, _state, _pool) = fixture().await;

    let res = app
        .oneshot(get_anon(&path_for("any-uuid")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_external_ratings_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;

    let res = get_external_ratings(fake_user(1), State(state), Path("any-uuid".into())).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// ── POST ─────────────────────────────────────────────────────────

#[tokio::test]
async fn api_post_external_ratings_requires_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Rated").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post(
            &path_for(&uuid),
            &token,
            serde_json::json!({ "isbn13": ISBN13 }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(body_string(res).await.contains("edit permission required"));
}

#[tokio::test]
async fn api_post_external_ratings_requires_auth() {
    let (app, _state, _pool) = fixture().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(path_for("any-uuid"))
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"isbn13":"9780134685991"}"#))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_external_ratings_rejects_a_malformed_isbn() {
    // Rejected before any provider is contacted — the value is interpolated
    // into provider queries.
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Rated").await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            &path_for(&uuid),
            &token,
            serde_json::json!({ "isbn13": "not-an-isbn" }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_external_ratings_rejects_a_bad_check_digit() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Rated").await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            &path_for(&uuid),
            &token,
            // Same digits as `ISBN13` with the check digit flipped, so length
            // and character-class checks alone wouldn't catch it.
            serde_json::json!({ "isbn13": "9780134685990" }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_post_external_ratings_returns_404_for_an_unindexed_book() {
    // The book is resolved before the fan-out, so this never reaches a
    // provider — which is also what keeps this test network-free.
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "editor").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post(
            &path_for("no-such-uuid"),
            &token,
            serde_json::json!({ "isbn13": ISBN13 }),
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert!(body_string(res).await.contains("book not found"));
}

#[tokio::test]
async fn api_post_external_ratings_returns_500_when_db_unavailable() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;

    let res = post_external_ratings(
        fake_editor(1),
        State(state),
        Path("any-uuid".into()),
        Json(RefreshRatingsRequest {
            isbn13: ISBN13.to_string(),
        }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
