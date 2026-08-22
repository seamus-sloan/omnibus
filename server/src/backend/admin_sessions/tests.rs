//! Integration tests for the admin device & session management REST
//! surface: AdminUser gating (401 anon / 403 non-admin), list sessions
//! / devices for a target user, and revoke a session / device (with the
//! matching credential invalidation and 404 on an unknown id).

use axum::{
    body::{to_bytes, Body},
    extract::{Path, State},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db::auth::SessionKind;
use omnibus_shared::{DeviceView, SessionView};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::auth::{AdminUser, AuthUser};
use crate::backend::test_support::*;

/// A minimal admin `AuthUser` for driving the handlers directly (bypassing
/// the `AdminUser` extractor). Needed because each handler's own
/// DB-failure branch shares its target table (`sessions`/`devices`) with
/// the extractor's own session lookup, so closing the pool before a
/// routed request would fail extraction rather than the handler body
/// under test.
fn fake_admin(id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "admin".to_string(),
        is_admin: true,
        can_upload: true,
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

fn req(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
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

// ── Gating ───────────────────────────────────────────────────────

#[tokio::test]
async fn get_user_sessions_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(anon("GET", "/api/admin/users/1/sessions"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_user_sessions_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/sessions", user.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn get_user_devices_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/devices", user.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_session_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req("DELETE", "/api/admin/sessions/1", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn delete_device_returns_403_for_non_admin() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(req("DELETE", "/api/admin/devices/1", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

// ── List (admin-success) ─────────────────────────────────────────

#[tokio::test]
async fn admin_lists_a_target_users_sessions_and_devices() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let target = auth_test_support::create_user(&pool, "bob").await;
    // Bob's own session (independent of any admin session).
    let bobs_session =
        omnibus_db::auth::create_session(&pool, target.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
    omnibus_db::auth::register_device(&pool, target.id, "Bob's Kobo", "web", None)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/sessions", target.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let sessions: Vec<SessionView> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, bobs_session.session.id);
    assert!(
        !sessions[0].is_current,
        "an admin's own request session is never the row being viewed"
    );

    let res = app
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/devices", target.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let devices: Vec<DeviceView> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "Bob's Kobo");
}

// ── Revoke session (admin-success + AC3 + 404) ────────────────────

#[tokio::test]
async fn admin_revokes_a_users_session_and_its_credential_stops_authenticating() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let target = auth_test_support::create_user(&pool, "bob").await;
    let bobs_token = auth_test_support::bearer_token(&pool, target.id).await;
    let (_user, session) = omnibus_db::auth::lookup_session(&pool, &bobs_token)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/admin/sessions/{}", session.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/sessions", target.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let sessions: Vec<SessionView> = serde_json::from_slice(&bytes).unwrap();
    assert!(
        sessions.is_empty(),
        "revoked session must no longer show as live"
    );
}

#[tokio::test]
async fn admin_revoke_session_returns_404_for_unknown_id() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req("DELETE", "/api/admin/sessions/999999", &admin_tok))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Mirrors `auth::handlers::delete_session_handler`'s self-lockout guard:
/// an admin revoking the session their own request authenticated with is
/// rejected with `400`, not honored.
#[tokio::test]
async fn admin_revoke_session_returns_400_when_targeting_own_current_session() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let (_user, own_session) = omnibus_db::auth::lookup_session(&pool, &admin_tok)
        .await
        .unwrap();

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/admin/sessions/{}", own_session.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // The session must still be live — the guard rejected before revoking.
    // `/api/auth/me` lives on the separate `auth_router` (server/src/auth/
    // handlers.rs), only merged with `rest_router` in main.rs, so this
    // fixture's app can't route it; check liveness directly against the
    // pool instead, matching the pattern the sibling revoke test above
    // uses.
    omnibus_db::auth::lookup_session(&pool, &admin_tok)
        .await
        .expect("own session must remain usable after the rejected self-revoke");
}

/// The guard is scoped to the requester's *own* session id — an admin can
/// still revoke a different admin's current session (only self-lockout is
/// refused, matching the self-service handler's behavior).
#[tokio::test]
async fn admin_revokes_another_admins_current_session() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let other_admin_tok = admin_token(&pool, "carol").await;
    let (_user, other_session) = omnibus_db::auth::lookup_session(&pool, &other_admin_tok)
        .await
        .unwrap();

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/admin/sessions/{}", other_session.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

// ── Revoke device (admin-success + cascades sessions + 404) ───────

#[tokio::test]
async fn admin_revokes_a_users_device_and_its_live_session() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let target = auth_test_support::create_user(&pool, "bob").await;
    let device = omnibus_db::auth::register_device(&pool, target.id, "Bob's Phone", "ios", None)
        .await
        .unwrap();
    let tied = omnibus_db::auth::create_session(
        &pool,
        target.id,
        Some(device.id),
        SessionKind::Bearer,
        3600,
    )
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(req(
            "DELETE",
            &format!("/api/admin/devices/{}", device.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let err = omnibus_db::auth::lookup_session(&pool, &tied.raw_token)
        .await
        .unwrap_err();
    assert!(matches!(err, omnibus_db::auth::AuthError::SessionNotFound));

    let res = app
        .oneshot(req(
            "GET",
            &format!("/api/admin/users/{}/devices", target.id),
            &admin_tok,
        ))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let devices: Vec<DeviceView> = serde_json::from_slice(&bytes).unwrap();
    assert!(devices.is_empty());
}

#[tokio::test]
async fn admin_revoke_device_returns_404_for_unknown_id() {
    let (app, _, pool) = fixture().await;
    let admin_tok = admin_token(&pool, "alice").await;
    let res = app
        .oneshot(req("DELETE", "/api/admin/devices/999999", &admin_tok))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ── DB-failure paths (500) ─────────────────────────────────────────
//
// Each handler is invoked directly (rather than through `oneshot`) with a
// hand-built `AdminUser` and a pool closed only after the point at which
// the `AdminUser` extractor would have already run — closing the pool
// before a routed request would fail extraction itself (same `sessions`
// table), not the handler body these tests target.

#[tokio::test]
async fn get_user_sessions_returns_500_when_pool_closed() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = get_user_sessions(AdminUser(fake_admin(1)), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_user_devices_returns_500_when_pool_closed() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = get_user_devices(AdminUser(fake_admin(1)), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_session_returns_500_when_pool_closed() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    // Target id (2) deliberately differs from `fake_admin`'s own
    // `session_id` (1) so the self-lockout guard below doesn't
    // short-circuit before reaching the DB call this test targets.
    let res = delete_session(AdminUser(fake_admin(1)), State(state), Path(2)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_device_returns_500_when_pool_closed() {
    let (_app, state, pool) = fixture().await;
    pool.close().await;
    let res = delete_device(AdminUser(fake_admin(1)), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
