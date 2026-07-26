//! Integration tests for the admin user-management REST surface: AdminUser
//! gating (401 anon / 403 non-admin), create (201 / 409 dup / 422 policy),
//! permission edits (204 / 409 last-admin / 404), password reset, unlock, and
//! delete (with the last-admin guard).

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::AdminUserRow;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let mut b = Request::builder()
        .uri(uri)
        .method(method)
        .header(AUTHORIZATION, format!("Bearer {token}"));
    match body {
        Some(json) => {
            b = b.header("content-type", "application/json");
            b.body(Body::from(json.to_string())).unwrap()
        }
        None => b.body(Body::empty()).unwrap(),
    }
}

fn anon(method: &str, uri: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .body(Body::empty())
        .unwrap()
}

async fn admin_token(pool: &sqlx::SqlitePool, name: &str) -> String {
    let admin = auth_test_support::create_admin(pool, name).await;
    auth_test_support::bearer_token(pool, admin.id).await
}

async fn list_rows(app: &axum::Router, token: &str) -> Vec<AdminUserRow> {
    let res = app
        .clone()
        .oneshot(req("GET", "/api/users", token, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn create_body(username: &str, password: &str, is_admin: bool) -> serde_json::Value {
    serde_json::json!({
        "username": username,
        "password": password,
        "permissions": {
            "is_admin": is_admin,
            "can_upload": is_admin,
            "can_edit": is_admin,
            "can_download": true,
        },
    })
}

// ── Gating ───────────────────────────────────────────────────────

#[tokio::test]
async fn list_users_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(anon("GET", "/api/users")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn list_users_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req("GET", "/api/users", &token, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_user_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req(
            "POST",
            "/api/users",
            &token,
            Some(create_body("bob", "bunker9-longer-pass", false)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

// ── List / create ────────────────────────────────────────────────

#[tokio::test]
async fn admin_lists_and_creates_users() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;

    // Only alice exists at first.
    let rows = list_rows(&app, &token).await;
    assert_eq!(rows.len(), 1);
    assert!(rows[0].is_admin);

    let res = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/users",
            &token,
            Some(create_body("bob", "bunker9-longer-pass", false)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let created: AdminUserRow = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(created.username, "bob");
    assert!(!created.is_admin && created.can_download);

    let rows = list_rows(&app, &token).await;
    assert_eq!(rows.len(), 2);
}

#[tokio::test]
async fn create_user_returns_409_on_duplicate_username() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req(
            "POST",
            "/api/users",
            &token,
            Some(create_body("Alice", "bunker9-longer-pass", false)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn create_user_returns_422_on_weak_password() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req(
            "POST",
            "/api/users",
            &token,
            Some(create_body("bob", "short", false)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── Permissions ──────────────────────────────────────────────────

#[tokio::test]
async fn patch_permissions_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(anon("PATCH", "/api/users/1/permissions"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn patch_permissions_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let res = app
        .oneshot(req(
            "PATCH",
            &format!("/api/users/{}/permissions", bob.id),
            &token,
            Some(serde_json::json!({
                "is_admin": false, "can_upload": true, "can_edit": true, "can_download": false,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn patch_permissions_updates_flags() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;

    let res = app
        .clone()
        .oneshot(req(
            "PATCH",
            &format!("/api/users/{}/permissions", bob.id),
            &token,
            Some(serde_json::json!({
                "is_admin": false, "can_upload": true, "can_edit": true, "can_download": false,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let rows = list_rows(&app, &token).await;
    let bob_row = rows.iter().find(|u| u.id == bob.id).unwrap();
    assert!(bob_row.can_upload && bob_row.can_edit && !bob_row.can_download);
}

#[tokio::test]
async fn patch_permissions_returns_409_demoting_last_admin() {
    let (app, _, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let res = app
        .oneshot(req(
            "PATCH",
            &format!("/api/users/{}/permissions", admin.id),
            &token,
            Some(serde_json::json!({
                "is_admin": false, "can_upload": false, "can_edit": false, "can_download": true,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn patch_permissions_returns_404_for_unknown_user() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req(
            "PATCH",
            "/api/users/9999/permissions",
            &token,
            Some(serde_json::json!({
                "is_admin": false, "can_upload": false, "can_edit": false, "can_download": true,
            })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── Password reset ───────────────────────────────────────────────

#[tokio::test]
async fn post_password_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(anon("POST", "/api/users/1/password"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_password_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let res = app
        .oneshot(req(
            "POST",
            &format!("/api/users/{}/password", bob.id),
            &token,
            Some(serde_json::json!({ "password": "reset-by-admin-99" })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn reset_password_succeeds_and_rejects_weak() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;

    let ok = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/users/{}/password", bob.id),
            &token,
            Some(serde_json::json!({ "password": "reset-by-admin-99" })),
        ))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::NO_CONTENT);

    let weak = app
        .oneshot(req(
            "POST",
            &format!("/api/users/{}/password", bob.id),
            &token,
            Some(serde_json::json!({ "password": "short" })),
        ))
        .await
        .unwrap();
    assert_eq!(weak.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

// ── Unlock ───────────────────────────────────────────────────────

#[tokio::test]
async fn post_unlock_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(anon("POST", "/api/users/1/unlock"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn post_unlock_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let res = app
        .oneshot(req(
            "POST",
            &format!("/api/users/{}/unlock", bob.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn unlock_clears_lockout() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    sqlx::query("UPDATE users SET failed_login_count = 5, locked_until = strftime('%s','now') + 3600 WHERE id = ?")
        .bind(bob.id)
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(req(
            "POST",
            &format!("/api/users/{}/unlock", bob.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let rows = list_rows(&app, &token).await;
    assert!(!rows.iter().find(|u| u.id == bob.id).unwrap().locked);
}

// ── Delete ───────────────────────────────────────────────────────

#[tokio::test]
async fn delete_user_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(anon("DELETE", "/api/users/1")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_user_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/users/{}", bob.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_user_succeeds() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;

    let res = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/users/{}", bob.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let rows = list_rows(&app, &token).await;
    assert!(rows.iter().all(|u| u.id != bob.id));
}

#[tokio::test]
async fn delete_user_returns_409_for_last_admin() {
    let (app, _, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;
    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/users/{}", admin.id),
            &token,
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn delete_user_returns_404_for_unknown() {
    let (app, _, pool) = fixture().await;
    let token = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req("DELETE", "/api/users/9999", &token, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
