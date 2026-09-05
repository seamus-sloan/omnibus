//! The unauthenticated registration-status read (open, closed by an
//! admin, the settings-read failure) with the matching register refusal,
//! and the per-account preferences `me` reports.

use axum::{
    body::Body,
    http::{header, Request},
};
use omnibus_db as db;
use serde_json::json;
use tower::ServiceExt;

use super::super::*;
use super::{app, json_req};

/// Read `enabled` out of a `GET /api/auth/registration` response.
async fn registration_enabled_over_http(app: &Router) -> bool {
    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/registration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice::<omnibus_shared::RegistrationStatus>(&body)
        .expect("valid RegistrationStatus body")
        .enabled
}

#[tokio::test]
async fn registration_status_is_readable_without_a_session() {
    // No Authorization header and no cookie: the login and register pages
    // read this before any session exists, so it must answer anonymously.
    let (app, pool) = app().await;
    db::auth::set_registration_enabled(&pool, true)
        .await
        .unwrap();

    assert!(registration_enabled_over_http(&app).await);
}

#[tokio::test]
async fn registration_status_reports_disabled_after_admin_closes_it() {
    let (app, pool) = app().await;
    db::auth::set_registration_enabled(&pool, false)
        .await
        .unwrap();

    assert!(!registration_enabled_over_http(&app).await);
}

#[tokio::test]
async fn register_is_refused_with_403_when_registration_is_disabled() {
    // The status endpoint is advisory; this is the real gate. A client that
    // ignores the closed state still cannot create an account.
    let (app, pool) = app().await;
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .expect("first user always allowed");
    db::auth::set_registration_enabled(&pool, false)
        .await
        .unwrap();

    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "bob", "password": "correct horse battery staple"}),
        ))
        .await
        .expect("request should succeed");

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn registration_status_returns_500_when_the_settings_read_fails() {
    let (app, pool) = app().await;
    sqlx::query("DROP TABLE settings")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/registration")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("request should succeed");

    // Must not fall open to `{"enabled": true}` on a broken read — that would
    // advertise signup on a server that cannot tell whether it is allowed.
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn me_reports_saved_hidden_formats() {
    let (app, pool) = app().await;
    let user = crate::auth::test_support::create_user(&pool, "reader").await;
    let token = crate::auth::test_support::bearer_token(&pool, user.id).await;
    db::auth::set_hidden_formats(&pool, user.id, &["cbz".into()])
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: omnibus_shared::UserSummary = serde_json::from_slice(&body).unwrap();
    assert_eq!(me.hidden_formats, vec!["cbz".to_string()]);
}

#[tokio::test]
async fn me_reports_saved_book_detail_scroll_stops() {
    let (app, pool) = app().await;
    let user = crate::auth::test_support::create_user(&pool, "reader").await;
    let token = crate::auth::test_support::bearer_token(&pool, user.id).await;
    db::auth::set_book_detail_scroll_stops(&pool, user.id, true)
        .await
        .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let me: omnibus_shared::UserSummary = serde_json::from_slice(&body).unwrap();
    assert!(me.book_detail_scroll_stops);
}
