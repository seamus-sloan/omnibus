//! Tests for the Send-to-Kindle REST handlers: admin gating on the SMTP
//! config, per-user Kindle-email set/validate, and the send gates (missing
//! email → 422, missing book → 404). The happy-path SMTP delivery is not
//! exercised here — it needs a live relay and belongs to the E2E suite.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use super::*;
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

fn smtp_body() -> serde_json::Value {
    serde_json::json!({
        "host": "smtp.example.com",
        "port": 587,
        "username": "postmaster",
        "from_email": "library@example.com",
        "security": "starttls",
        "password": "s3cret-pass",
    })
}

/// Configure SMTP directly in the DB so the send/test gates pass regardless of
/// process env (settings wins over `SMTP_*`).
async fn configure_smtp(pool: &sqlx::SqlitePool) {
    db::set_smtp_config(
        pool,
        &omnibus_shared::SmtpConfigUpdate {
            host: "smtp.example.com".into(),
            port: 587,
            username: "postmaster".into(),
            from_email: "library@example.com".into(),
            security: omnibus_shared::SmtpSecurity::Starttls,
            password: Some("s3cret-pass".into()),
        },
    )
    .await
    .unwrap();
}

// ── SMTP config admin surface ────────────────────────────────────

#[tokio::test]
async fn smtp_get_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/smtp")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn smtp_get_returns_403_when_not_admin() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/smtp", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn smtp_post_saves_and_returns_masked_status() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let response = app
        .oneshot(post_json("/api/smtp", &token, smtp_body()))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let status: omnibus_shared::SmtpConfigStatus = serde_json::from_slice(&bytes).unwrap();
    assert!(status.configured);
    assert_eq!(status.host.as_deref(), Some("smtp.example.com"));
    // The raw password must never be echoed.
    let raw = std::str::from_utf8(&bytes).unwrap();
    assert!(!raw.contains("s3cret-pass"), "password leaked: {raw}");
}

#[tokio::test]
async fn smtp_post_returns_422_for_invalid_from_email() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let mut body = smtp_body();
    body["from_email"] = serde_json::json!("not-an-email");
    let response = app
        .oneshot(post_json("/api/smtp", &token, body))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn smtp_test_returns_422_when_admin_has_no_kindle_email() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    configure_smtp(&pool).await;

    let response = app
        .oneshot(post_json("/api/smtp/test", &token, serde_json::json!({})))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── Per-user Kindle email ────────────────────────────────────────

#[tokio::test]
async fn kindle_email_post_sets_and_persists() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(post_json(
            "/api/account/kindle-email",
            &token,
            serde_json::json!({ "email": "reader@kindle.com" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let stored = db::auth::get_kindle_email(&pool, user.id).await.unwrap();
    assert_eq!(stored.as_deref(), Some("reader@kindle.com"));
}

#[tokio::test]
async fn kindle_email_post_returns_422_for_invalid_address() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(post_json(
            "/api/account/kindle-email",
            &token,
            serde_json::json!({ "email": "nope" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn kindle_email_post_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/account/kindle-email")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"email":"x@y.com"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ── Send gates ───────────────────────────────────────────────────

#[tokio::test]
async fn send_returns_422_when_user_has_no_kindle_email() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(post_json(
            "/api/kindle/send",
            &token,
            serde_json::json!({ "book_uuid": "missing" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn send_returns_404_when_gates_pass_but_book_missing() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::auth::set_kindle_email(&pool, user.id, Some("reader@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;

    let response = app
        .oneshot(post_json(
            "/api/kindle/send",
            &token,
            serde_json::json!({ "book_uuid": "does-not-exist" }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(
            Request::builder()
                .uri("/api/kindle/send")
                .method("POST")
                .header("content-type", "application/json")
                .body(Body::from(r#"{"book_uuid":"x"}"#))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn send_enqueues_and_returns_task_id_when_gates_pass() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::auth::set_kindle_email(&pool, user.id, Some("reader@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Sendable").await;

    let response = app
        .oneshot(post_json(
            "/api/kindle/send",
            &token,
            serde_json::json!({ "book_uuid": uuid }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        body["task_id"].as_u64().is_some(),
        "expected a numeric task_id, got {body}"
    );
}

#[tokio::test]
async fn send_status_is_not_readable_by_another_user() {
    // A user enqueues a send; a *different* authenticated user must not be
    // able to read that task's status by guessing the id (the id space is a
    // monotonic AtomicU64). The owner still sees it.
    let (app, _state, pool) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    db::auth::set_kindle_email(&pool, owner.id, Some("owner@kindle.com"))
        .await
        .unwrap();
    configure_smtp(&pool).await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Sendable").await;

    let response = app
        .clone()
        .oneshot(post_json(
            "/api/kindle/send",
            &owner_token,
            serde_json::json!({ "book_uuid": uuid }),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let task_id = serde_json::from_slice::<serde_json::Value>(&bytes).unwrap()["task_id"]
        .as_u64()
        .expect("task_id");

    // A second user probing the same id gets 404, not the owner's outcome.
    let other = auth_test_support::create_user(&pool, "snoop").await;
    let other_token = auth_test_support::bearer_token(&pool, other.id).await;
    let probed = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/kindle/send/status?task_id={task_id}"),
            &other_token,
        ))
        .await
        .unwrap();
    assert_eq!(probed.status(), StatusCode::NOT_FOUND);

    // The owner can still read their own send's status.
    let owned = app
        .oneshot(get_with_bearer(
            &format!("/api/kindle/send/status?task_id={task_id}"),
            &owner_token,
        ))
        .await
        .unwrap();
    assert_eq!(owned.status(), StatusCode::OK);
}

#[tokio::test]
async fn send_status_returns_404_for_unknown_task() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer(
            "/api/kindle/send/status?task_id=999999",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn send_status_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let response = app
        .oneshot(get_anon("/api/kindle/send/status?task_id=1"))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
