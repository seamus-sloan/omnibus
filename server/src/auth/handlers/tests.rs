//! Tests for auth register/login/logout/me handlers.
use axum::{
    body::Body,
    http::{header, Request},
};
use omnibus_db as db;
use serde_json::json;
use tower::ServiceExt;

use super::*;

async fn app() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let router = auth_router(AppState::new(pool.clone()));
    (router, pool)
}

fn json_req(uri: &str, method: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[test]
fn parse_secure_cookies_defaults_on_when_unset() {
    assert!(parse_secure_cookies(None));
}

#[test]
fn parse_secure_cookies_off_for_falsey_values() {
    for raw in ["0", "false", "FALSE", "no", " 0 ", ""] {
        assert!(
            !parse_secure_cookies(Some(raw)),
            "expected {raw:?} to disable Secure"
        );
    }
}

#[test]
fn parse_secure_cookies_on_for_anything_else() {
    for raw in ["1", "true", "yes", "anything"] {
        assert!(
            parse_secure_cookies(Some(raw)),
            "expected {raw:?} to keep Secure on"
        );
    }
}

#[test]
fn session_cookie_uses_host_prefix_when_secure() {
    // `__Host-` is required for the subdomain-injection guarantee;
    // verify both the chosen name and the attributes that the prefix
    // contract demands (Secure, Path=/, no Domain).
    let c = session_cookie("tok".to_string(), 60);
    // secure_cookies() defaults to true (no env override in test).
    assert_eq!(c.name(), crate::auth::SESSION_COOKIE_HOST_PREFIXED);
    assert_eq!(c.path(), Some("/"));
    assert!(c.secure().unwrap_or(false));
    assert!(c.domain().is_none());
}

#[test]
fn session_cookie_name_helper_branches_on_secure_flag() {
    assert_eq!(super::session_cookie_name(true), "__Host-omnibus_session");
    assert_eq!(super::session_cookie_name(false), "omnibus_session");
}

#[tokio::test]
async fn register_first_user_becomes_admin_and_sets_cookie() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let set_cookie: Vec<_> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    // Default is secure_cookies() == true, so the cookie ships under the
    // `__Host-` prefixed name. The plain name is used only when
    // OMNIBUS_SECURE_COOKIES is explicitly disabled for plain-HTTP LAN dev.
    let expected_name = format!("{}=", super::session_cookie_name(true));
    assert!(
        set_cookie.iter().any(|c| c.starts_with(&expected_name)),
        "expected Set-Cookie starting with {expected_name:?}, got {set_cookie:?}",
    );
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: LoginResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.user.username, "alice");
    assert!(body.user.is_admin);
    assert!(body.token.is_none());
}

#[tokio::test]
async fn register_bearer_returns_token() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({
                "username": "bob",
                "password": "correct horse battery staple",
                "client_kind": "ios",
                "device_name": "Bob's iPhone"
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: LoginResponse = serde_json::from_slice(&bytes).unwrap();
    assert!(body.token.is_some(), "bearer flow must return token");
}

#[tokio::test]
async fn register_short_password_returns_400() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "alice", "password": "short"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn register_invalid_username_returns_400() {
    // Cover each AuthError::Validation username rejection at the HTTP
    // boundary (#276). Each case is a fresh app so the
    // first-registration / registration-disabled gate doesn't latch
    // between iterations.
    // `too_long` deliberately uses 256 chars — well over the 64-scalar
    // MAX_USERNAME_LEN policy without depending on the (private) const.
    let too_long = "x".repeat(256);
    let cases: &[(&str, &str)] = &[
        ("empty", ""),
        ("too_long", &too_long),
        ("leading_space", " alice"),
        ("trailing_space", "alice "),
        ("control_char", "ali\x01ce"),
        ("null_byte", "ali\0ce"),
    ];
    for (label, username) in cases {
        let (app, _pool) = app().await;
        let res = app
            .oneshot(json_req(
                "/api/auth/register",
                "POST",
                json!({"username": username, "password": "correct horse battery staple"}),
            ))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::BAD_REQUEST,
            "username case {label} must reject with 400, got {}",
            res.status()
        );
    }
}

#[tokio::test]
async fn register_second_same_username_returns_409() {
    let (app, pool) = app().await;
    // First registration closes the gate; reopen it so the second attempt
    // exercises the UsernameTaken path instead of RegistrationDisabled.
    let _ = app
        .clone()
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    db::auth::set_registration_enabled(&pool, true)
        .await
        .unwrap();
    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn register_when_disabled_returns_403() {
    let (app, _pool) = app().await;
    let _ = app
        .clone()
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    let res = app
        .oneshot(json_req(
            "/api/auth/register",
            "POST",
            json!({"username": "bob", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn login_with_valid_credentials_returns_200_and_set_cookie() {
    let (app, pool) = app().await;
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    let res = app
        .oneshot(json_req(
            "/api/auth/login",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // The response must include a Set-Cookie header with the session
    // cookie. Under the default OMNIBUS_SECURE_COOKIES=true env the
    // name uses the `__Host-` prefix.
    let set_cookie: Vec<_> = res
        .headers()
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    let expected_name = format!("{}=", super::session_cookie_name(true));
    assert!(
        set_cookie.iter().any(|c| c.starts_with(&expected_name)),
        "expected Set-Cookie starting with {expected_name:?}, got {set_cookie:?}",
    );

    // Bearer flow must NOT be active: token field must be absent.
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    let body: LoginResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body.user.username, "alice");
    assert!(
        body.token.is_none(),
        "cookie flow must not return a bearer token in the body"
    );
}

#[tokio::test]
async fn login_wrong_password_returns_401() {
    let (app, pool) = app().await;
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    let res = app
        .oneshot(json_req(
            "/api/auth/login",
            "POST",
            json!({"username": "alice", "password": "nope"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_after_account_lockout_returns_429_with_retry_after() {
    // After `LOCKOUT_MIN_AFTER` (5) consecutive failed attempts the DB
    // sets `locked_until`; `auth_error_to_response` must map
    // `AccountLocked` to 429 with a `Retry-After` header.
    let (app, pool) = app().await;
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();

    // Exhaust the lockout threshold with wrong-password requests. Each
    // attempt re-uses `app.clone()` because `oneshot` consumes the router.
    for _ in 0..5 {
        app.clone()
            .oneshot(json_req(
                "/api/auth/login",
                "POST",
                json!({"username": "alice", "password": "wrong"}),
            ))
            .await
            .unwrap();
    }

    // The next attempt — even with the correct password — must be locked out.
    let res = app
        .oneshot(json_req(
            "/api/auth/login",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
    assert!(
        res.headers().contains_key(header::RETRY_AFTER),
        "locked-out response must include a Retry-After header"
    );
    // The body must use the same generic message as a wrong password so
    // the response doesn't confirm the username exists.
    let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(bytes.as_ref(), b"invalid credentials");
}

#[tokio::test]
async fn login_unknown_user_returns_401() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(json_req(
            "/api/auth/login",
            "POST",
            json!({"username": "ghost", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn login_corrupted_password_hash_returns_500() {
    // Regression: `AuthError::Crypto` (raised when the stored
    // `password_hash` isn't a valid PHC string) is mapped to a generic
    // 500 by `auth_error_to_response`, same as `Internal`. Locks in that
    // a corrupted row surfaces as a deliberate internal error rather than
    // a panic or a silent auth bypass.
    let (app, pool) = app().await;
    let user = db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind("not-a-valid-phc-string")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let res = app
        .oneshot(json_req(
            "/api/auth/login",
            "POST",
            json!({"username": "alice", "password": "correct horse battery staple"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn logout_revokes_session_and_next_me_is_401() {
    let (app, pool) = app().await;
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    let user = db::auth::get_user_by_username(&pool, "alice")
        .await
        .unwrap()
        .unwrap();
    let issued = db::auth::create_session(&pool, user.id, None, SessionKind::Bearer, 3600)
        .await
        .unwrap();

    let res = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/api/auth/logout")
                .method("POST")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", issued.raw_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth/me")
                .header(
                    header::AUTHORIZATION,
                    format!("Bearer {}", issued.raw_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// ── GET /api/auth/registration (public read) ──────────────────────────

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

// ── GET/DELETE /api/auth/sessions (self-service, AC2/AC3/AC4) ──────────

fn bearer_req(method: &str, uri: &str, token: &str) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method(method)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap()
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
    let other_alice = db::auth::create_session(&pool, alice.id, None, SessionKind::Bearer, 3600)
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
    let other = db::auth::create_session(&pool, alice.id, None, SessionKind::Bearer, 3600)
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
    let bobs_session = db::auth::create_session(&pool, bob.id, None, SessionKind::Bearer, 3600)
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
