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
