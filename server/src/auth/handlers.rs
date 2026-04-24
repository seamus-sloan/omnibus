//! `/api/auth/*` handlers plus the small router that mounts them.
//!
//! Cookie sessions (web) vs bearer sessions (mobile) are selected by
//! `client_kind` in the request body: `ios`/`android`/`bearer` → bearer
//! session, token in the JSON response; anything else (or unset) → cookie
//! session, token in `Set-Cookie`.

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use omnibus_db::auth::{self as auth_db, AuthError, SessionKind};
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};

use super::extractor::{extract_token, AuthUser};
use super::{BEARER_TTL_SECS, COOKIE_TTL_SECS, SESSION_COOKIE};
use crate::backend::AppState;

pub fn auth_router(state: AppState) -> Router {
    Router::new()
        .route("/api/auth/register", post(register_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/logout", post(logout_handler))
        .route("/api/auth/me", get(me_handler))
        .with_state(state)
}

fn user_summary(u: &auth_db::User) -> UserSummary {
    UserSummary {
        id: u.id,
        username: u.username.clone(),
        is_admin: u.is_admin,
        can_upload: u.can_upload,
        can_edit: u.can_edit,
        can_download: u.can_download,
    }
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    tracing::error!(error = %e, "internal auth error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

fn auth_error_to_response(e: AuthError) -> Response {
    match e {
        AuthError::InvalidCredentials => {
            (StatusCode::UNAUTHORIZED, "invalid credentials").into_response()
        }
        // Don't confirm the username exists: return the same generic
        // "invalid credentials" body as a wrong password, but with 429 and
        // a `Retry-After` header so a well-behaved client can back off.
        AuthError::AccountLocked { until_unix } => {
            let now_unix = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            let retry_after = until_unix.saturating_sub(now_unix).max(0);
            let mut headers = HeaderMap::new();
            if let Ok(v) = retry_after.to_string().parse() {
                headers.insert(header::RETRY_AFTER, v);
            }
            (
                StatusCode::TOO_MANY_REQUESTS,
                headers,
                "invalid credentials",
            )
                .into_response()
        }
        AuthError::UsernameTaken => (StatusCode::CONFLICT, "username taken").into_response(),
        AuthError::PasswordTooShort { min } => (
            StatusCode::BAD_REQUEST,
            format!("password too short (min {min})"),
        )
            .into_response(),
        AuthError::PasswordTooLong { max } => (
            StatusCode::BAD_REQUEST,
            format!("password too long (max {max})"),
        )
            .into_response(),
        AuthError::PasswordCommon => {
            (StatusCode::BAD_REQUEST, "password is too common").into_response()
        }
        AuthError::RegistrationDisabled => {
            (StatusCode::FORBIDDEN, "registration disabled").into_response()
        }
        AuthError::SessionNotFound => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        AuthError::Db(e) => internal(e),
        AuthError::Hash(e) => internal(e),
    }
}

fn want_bearer(kind: Option<&str>) -> bool {
    matches!(kind, Some("ios") | Some("android") | Some("bearer"))
}

fn secure_cookies() -> bool {
    match std::env::var("OMNIBUS_SECURE_COOKIES") {
        Ok(v) => {
            let v = v.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "no" | "")
        }
        Err(_) => false, // default off so the dev shell on http://localhost works
    }
}

fn session_cookie(value: String, max_age_secs: i64) -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, value);
    c.set_http_only(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_secure(secure_cookies());
    c.set_max_age(time::Duration::seconds(max_age_secs));
    c
}

fn cleared_cookie() -> Cookie<'static> {
    let mut c = Cookie::new(SESSION_COOKIE, "");
    c.set_http_only(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_secure(secure_cookies());
    c.set_max_age(time::Duration::seconds(0));
    c
}

async fn register_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<RegisterRequest>,
) -> Response {
    let user = match auth_db::create_user(state.pool(), &req.username, &req.password).await {
        Ok(u) => u,
        Err(e) => return auth_error_to_response(e),
    };
    issue_session(
        &state,
        jar,
        user,
        req.client_kind,
        req.device_name,
        req.client_version,
    )
    .await
}

async fn login_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    Json(req): Json<LoginRequest>,
) -> Response {
    let user = match auth_db::verify_login(state.pool(), &req.username, &req.password).await {
        Ok(u) => u,
        Err(e) => return auth_error_to_response(e),
    };
    issue_session(
        &state,
        jar,
        user,
        req.client_kind,
        req.device_name,
        req.client_version,
    )
    .await
}

async fn issue_session(
    state: &AppState,
    jar: CookieJar,
    user: auth_db::User,
    client_kind: Option<String>,
    device_name: Option<String>,
    client_version: Option<String>,
) -> Response {
    let bearer = want_bearer(client_kind.as_deref());

    let device_id = if let (Some(name), Some(kind)) =
        (device_name.as_deref(), client_kind.as_deref())
    {
        match auth_db::register_device(state.pool(), user.id, name, kind, client_version.as_deref())
            .await
        {
            Ok(d) => Some(d.id),
            Err(e) => return auth_error_to_response(e),
        }
    } else {
        None
    };

    let (kind, ttl) = if bearer {
        (SessionKind::Bearer, BEARER_TTL_SECS)
    } else {
        (SessionKind::Cookie, COOKIE_TTL_SECS)
    };

    let issued = match auth_db::create_session(state.pool(), user.id, device_id, kind, ttl).await {
        Ok(s) => s,
        Err(e) => return auth_error_to_response(e),
    };

    let body = LoginResponse {
        user: user_summary(&user),
        token: if bearer {
            Some(issued.raw_token.clone())
        } else {
            None
        },
    };

    if bearer {
        (StatusCode::OK, Json(body)).into_response()
    } else {
        let jar = jar.add(session_cookie(issued.raw_token, ttl));
        (StatusCode::OK, jar, Json(body)).into_response()
    }
}

async fn logout_handler(
    State(state): State<AppState>,
    jar: CookieJar,
    headers: HeaderMap,
) -> Response {
    // Resolve the session from either cookie or bearer, revoke it, and
    // clear the cookie. Idempotent: unknown tokens still return 204.
    if let Some((token, _)) = extract_token(&headers, &jar) {
        match auth_db::lookup_session(state.pool(), &token).await {
            Ok((_user, session)) => {
                if let Err(e) = auth_db::revoke_session(state.pool(), session.id).await {
                    return internal(e);
                }
            }
            Err(AuthError::SessionNotFound) => {}
            Err(e) => return internal(e),
        }
    }
    let jar = jar.add(cleared_cookie());
    (StatusCode::NO_CONTENT, jar).into_response()
}

async fn me_handler(user: AuthUser) -> Response {
    Json(user.summary()).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{
        body::Body,
        http::{header, Request},
    };
    use omnibus_db as db;
    use serde_json::json;
    use tower::ServiceExt;

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
        assert!(set_cookie.iter().any(|c| c.starts_with("omnibus_session=")));
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
}
