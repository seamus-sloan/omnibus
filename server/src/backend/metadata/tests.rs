//! Tests for `GET /api/metadata/providers`.

use axum::{body::to_bytes, extract::State, http::StatusCode};
use omnibus_db::{auth::SessionKind, test_support::EnvVarGuard};
use tower::ServiceExt;

use super::{get_providers, AppState};
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

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), 1024 * 1024).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

#[tokio::test]
async fn api_get_metadata_providers_returns_the_catalog_for_an_authenticated_user() {
    // Pin both keys off so this test's expectations don't drift with a
    // developer's real `.env`.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None).also_set("GOOGLE_BOOKS_API_KEY", None);
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/metadata/providers", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let body: serde_json::Value = serde_json::from_str(&body_string(res).await).unwrap();
    let entries = body.as_array().expect("catalog should be a JSON array");
    assert_eq!(entries.len(), 3, "catalog should list all three providers");

    let hardcover = entries
        .iter()
        .find(|e| e["id"] == "hardcover")
        .expect("catalog should include hardcover");
    assert_eq!(hardcover["configured"], false);
    assert_eq!(hardcover["requires_key"], true);

    let open_library = entries
        .iter()
        .find(|e| e["id"] == "open_library")
        .expect("catalog should include open_library");
    assert_eq!(open_library["configured"], true);
}

#[tokio::test]
async fn api_get_metadata_providers_never_leaks_key_material() {
    let (app, _state, pool) = fixture().await;
    omnibus_db::set_hardcover_api_key(&pool, Some("hc_super_secret_value"))
        .await
        .unwrap();
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/metadata/providers", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(!body.contains("hc_super_secret_value"));
    assert!(!body.contains("masked"));
    assert!(!body.contains("source"));
}

#[tokio::test]
async fn api_get_metadata_providers_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/metadata/providers"))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_providers_returns_500_when_db_unavailable() {
    let (_app, _state, pool) = fixture().await;
    pool.close().await;
    let res = get_providers(fake_user(1), State(AppState::new(pool))).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
