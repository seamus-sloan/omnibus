//! The session list and revoke routes: auth, only the caller's sessions
//! with the current one flagged, revoking a non-current session, refusing
//! the current one, 404 for another user's or an unknown id, the client
//! name from the device or User-Agent, and the pool-closed 500s.

use axum::{
    body::Body,
    http::{header, Request},
};
use omnibus_db as db;
use serde_json::json;
use tower::ServiceExt;

use super::super::*;
use super::{app, json_req};

fn bearer_req(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
}

/// A minimal `AuthUser` for driving `get_sessions_handler` /
/// `delete_session_handler` directly (bypassing the `AuthUser` extractor).
/// Needed because both handlers' own DB-failure branch shares the
/// `sessions` table with the extractor's own session lookup: closing the
/// pool (or dropping the table) before the request would fail extraction
/// itself rather than the handler body under test, so the handler is
/// invoked directly with a hand-built caller and a pool closed only after
/// extraction would have happened.
fn fake_auth_user(id: i64, session_id: i64) -> AuthUser {
    AuthUser {
        id,
        username: "alice".to_string(),
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        book_detail_scroll_stops: false,
        session_id,
        session_kind: SessionKind::Bearer,
    }
}

#[tokio::test]
async fn get_sessions_handler_returns_500_when_pool_closed() {
    let (_app, pool) = app().await;
    pool.close().await;
    let state = AppState::new(pool);
    let res = get_sessions_handler(fake_auth_user(1, 1), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn delete_session_handler_returns_500_when_pool_closed() {
    let (_app, pool) = app().await;
    pool.close().await;
    let state = AppState::new(pool);
    // session_id (2) deliberately differs from the caller's own session_id
    // (1) so the handler's "cannot revoke the current session" 400 guard
    // doesn't short-circuit before the DB call under test runs.
    let res = delete_session_handler(fake_auth_user(1, 1), State(state), Path(2)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn get_sessions_returns_401_when_anonymous() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn get_sessions_lists_only_the_callers_own_sessions_and_flags_current() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let bob = crate::auth::test_support::create_user(&pool, "bob").await;
    let token = crate::auth::test_support::bearer_token(&pool, alice.id).await;
    // A second live session for alice, plus an unrelated session for bob —
    // neither should be omitted-or-leaked incorrectly.
    let other_alice =
        db::auth::create_session(&pool, alice.id, None, SessionKind::Bearer, 3600, None)
            .await
            .unwrap();
    let _bob_session = crate::auth::test_support::bearer_token(&pool, bob.id).await;

    let res = app
        .oneshot(bearer_req("GET", "/api/auth/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let sessions: Vec<omnibus_shared::SessionView> = serde_json::from_slice(&body).unwrap();
    assert_eq!(sessions.len(), 2, "must list both of alice's sessions");
    let current = sessions
        .iter()
        .find(|s| s.is_current)
        .expect("exactly one session must be flagged current");
    assert_ne!(
        current.id, other_alice.session.id,
        "the flagged session must be the one authenticating this request, not the other one"
    );
}

#[tokio::test]
async fn delete_session_revokes_a_non_current_session_and_subsequent_auth_fails() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let token = crate::auth::test_support::bearer_token(&pool, alice.id).await;
    let other = db::auth::create_session(&pool, alice.id, None, SessionKind::Bearer, 3600, None)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(bearer_req(
            "DELETE",
            &format!("/api/auth/sessions/{}", other.session.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    // AC3: the revoked session's own credential no longer authenticates.
    let res = app
        .oneshot(bearer_req("GET", "/api/auth/me", &other.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn delete_session_refuses_to_revoke_the_current_session() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let token = crate::auth::test_support::bearer_token(&pool, alice.id).await;

    // Resolve the session id the token maps to.
    let (_user, session) = db::auth::lookup_session(&pool, &token).await.unwrap();

    let res = app
        .clone()
        .oneshot(bearer_req(
            "DELETE",
            &format!("/api/auth/sessions/{}", session.id),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    // Still authenticated afterwards — the request session was never revoked.
    let res = app
        .oneshot(bearer_req("GET", "/api/auth/me", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_session_returns_404_for_another_users_session() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let bob = crate::auth::test_support::create_user(&pool, "bob").await;
    let alice_token = crate::auth::test_support::bearer_token(&pool, alice.id).await;
    let bobs_session =
        db::auth::create_session(&pool, bob.id, None, SessionKind::Bearer, 3600, None)
            .await
            .unwrap();

    let res = app
        .clone()
        .oneshot(bearer_req(
            "DELETE",
            &format!("/api/auth/sessions/{}", bobs_session.session.id),
            &alice_token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    // Bob's session must be untouched by alice's attempt.
    let res = app
        .oneshot(bearer_req("GET", "/api/auth/me", &bobs_session.raw_token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn delete_session_returns_404_for_unknown_id() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let token = crate::auth::test_support::bearer_token(&pool, alice.id).await;
    let res = app
        .oneshot(bearer_req("DELETE", "/api/auth/sessions/999999", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_sessions_names_the_client_from_the_login_user_agent() {
    let (app, pool) = app().await;
    crate::auth::test_support::create_user_with_password(&pool, "alice", "correct horse battery")
        .await;

    let mut login = json_req(
        "/api/auth/login",
        "POST",
        json!({
            "username": "alice",
            "password": "correct horse battery",
            "client_kind": "bearer",
        }),
    );
    login.headers_mut().insert(
        header::USER_AGENT,
        "Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) Gecko/20100101 Firefox/133.0"
            .parse()
            .unwrap(),
    );
    let res = app.clone().oneshot(login).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let issued: omnibus_shared::LoginResponse = serde_json::from_slice(&bytes).unwrap();
    let token = issued
        .token
        .expect("client_kind bearer must return a token");

    let res = app
        .oneshot(bearer_req("GET", "/api/auth/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let sessions: Vec<omnibus_shared::SessionView> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].client, "Firefox on macOS");
}

#[tokio::test]
async fn get_sessions_names_the_client_from_the_registered_device_when_one_exists() {
    let (app, pool) = app().await;
    crate::auth::test_support::create_user_with_password(&pool, "alice", "correct horse battery")
        .await;

    // A native client sends its own device name; that beats whatever the HTTP
    // stack happened to put in `User-Agent`.
    let mut login = json_req(
        "/api/auth/login",
        "POST",
        json!({
            "username": "alice",
            "password": "correct horse battery",
            "client_kind": "ios",
            "device_name": "Alice's iPhone",
        }),
    );
    login.headers_mut().insert(
        header::USER_AGENT,
        "omnibus/12 CFNetwork/1568.100.1 Darwin/24.1.0"
            .parse()
            .unwrap(),
    );
    let res = app.clone().oneshot(login).await.unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let issued: omnibus_shared::LoginResponse = serde_json::from_slice(&bytes).unwrap();
    let token = issued.token.expect("client_kind ios must return a token");

    let res = app
        .oneshot(bearer_req("GET", "/api/auth/sessions", &token))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let sessions: Vec<omnibus_shared::SessionView> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions[0].client, "Alice's iPhone");
}

#[tokio::test]
async fn get_sessions_falls_back_to_unknown_without_a_device_or_user_agent() {
    let (app, pool) = app().await;
    let alice = crate::auth::test_support::create_user(&pool, "alice").await;
    let token = crate::auth::test_support::bearer_token(&pool, alice.id).await;

    let res = app
        .oneshot(bearer_req("GET", "/api/auth/sessions", &token))
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let sessions: Vec<omnibus_shared::SessionView> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sessions[0].client, "Unknown client");
}
