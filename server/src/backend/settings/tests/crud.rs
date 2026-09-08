//! `GET`/`POST /api/settings`: the null defaults, the persisted round
//! trip including `scan_interval_hours`, the 401 / 403 gates, and the
//! 422s for over-long paths and a zero scan interval.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_settings_returns_null_defaults() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&body).unwrap();
    assert_eq!(settings.ebook_library_path, None);
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn api_post_settings_persists_and_returns_saved_values() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": "/books/ebooks",
        "audiobook_library_path": "/books/audio"
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        settings.ebook_library_path,
        Some("/books/ebooks".to_string())
    );
    assert_eq!(
        settings.audiobook_library_path,
        Some("/books/audio".to_string())
    );
}

#[tokio::test]
async fn api_get_settings_after_post_reflects_saved_values() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": "/my/ebooks",
        "audiobook_library_path": null
    });
    app.clone()
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("POST should succeed");

    let response = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .expect("GET should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(settings.ebook_library_path, Some("/my/ebooks".to_string()));
    assert_eq!(settings.audiobook_library_path, None);
}

#[tokio::test]
async fn api_get_settings_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/settings")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_settings_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": null,
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_settings_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/settings", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_post_settings_returns_422_when_ebook_path_exceeds_max_len() {
    // Validates the `settings.validate()` guard: an over-long path must
    // return 422 with the typed error message, and the on-disk settings
    // must remain untouched.
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "a".repeat(omnibus_shared::PATH_MAX_LEN + 1);
    let body = serde_json::json!({
        "ebook_library_path": over_limit,
        "audiobook_library_path": null,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        msg.contains("ebook_library_path"),
        "error body should name the offending field: {msg}"
    );
    assert!(
        msg.contains(&omnibus_shared::PATH_MAX_LEN.to_string()),
        "error body should name the limit: {msg}"
    );

    // 422 must short-circuit before `db::set_settings` runs.
    let stored = db::get_settings(&pool).await.expect("read settings");
    assert_eq!(stored.ebook_library_path, None);
    assert_eq!(stored.audiobook_library_path, None);
}

#[tokio::test]
async fn api_post_settings_returns_422_when_audiobook_path_exceeds_max_len() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let over_limit = "b".repeat(omnibus_shared::PATH_MAX_LEN + 1);
    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": over_limit,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        msg.contains("audiobook_library_path"),
        "error body should name the offending field: {msg}"
    );
}

#[tokio::test]
async fn api_post_settings_returns_422_when_scan_interval_hours_is_zero() {
    // Validates the `settings.validate()` guard added for the configurable
    // periodic scan (F): 0 is rejected rather than treated as "disable".
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": null,
        "scan_interval_hours": 0,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let msg = std::str::from_utf8(&bytes).unwrap_or("");
    assert!(
        msg.contains("scan_interval_hours"),
        "error body should name the offending field: {msg}"
    );

    // 422 must short-circuit before `db::set_settings` runs.
    let stored = db::get_settings(&pool).await.expect("read settings");
    assert_eq!(stored.scan_interval_hours, None);
}

#[tokio::test]
async fn api_post_settings_persists_and_returns_scan_interval_hours() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let body = serde_json::json!({
        "ebook_library_path": null,
        "audiobook_library_path": null,
        "scan_interval_hours": 12,
    });
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let settings: Settings = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(settings.scan_interval_hours, Some(12));

    let stored = db::get_settings(&pool).await.expect("read settings");
    assert_eq!(stored.scan_interval_hours, Some(12));
}

#[tokio::test]
async fn api_post_settings_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!({
        "ebook_library_path": "/evil/path",
        "audiobook_library_path": null,
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/settings")
                .method("POST")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
