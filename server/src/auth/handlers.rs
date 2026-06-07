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
    Extension, Json, Router,
};
use axum_extra::extract::cookie::{Cookie, CookieJar, SameSite};
use omnibus_db::auth::{self as auth_db, AuthError, SessionKind};
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};

use super::extractor::{extract_token, AuthUser};
use super::{session_cookie_name, BEARER_TTL_SECS, COOKIE_TTL_SECS};
use crate::backend::AppState;

/// Build the `/api/auth/{register,login,logout,me}` router.
pub fn auth_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/api/auth/register", post(register_handler))
        .route("/api/auth/login", post(login_handler))
        .route("/api/auth/logout", post(logout_handler))
        .route("/api/auth/me", get(me_handler))
        .with_state(state)
        // `AuthUser` reads the pool from `Extension<SqlitePool>` so it stays
        // state-agnostic. Keep this layer here so the router is usable
        // standalone (in integration tests) without relying on the
        // top-level Extension layer in `main.rs`.
        .layer(Extension(pool))
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
        AuthError::Validation(msg) => (StatusCode::BAD_REQUEST, msg).into_response(),
        AuthError::RegistrationDisabled => {
            (StatusCode::FORBIDDEN, "registration disabled").into_response()
        }
        AuthError::SessionNotFound => (StatusCode::UNAUTHORIZED, "unauthorized").into_response(),
        AuthError::Db(e) => internal(e),
        AuthError::Crypto(e) => internal(e),
    }
}

fn want_bearer(kind: Option<&str>) -> bool {
    matches!(kind, Some("ios") | Some("android") | Some("bearer"))
}

/// Parse the `OMNIBUS_SECURE_COOKIES` env value (defaults to `true` when unset).
pub fn parse_secure_cookies(raw: Option<&str>) -> bool {
    match raw {
        Some(v) => {
            let v = v.trim().to_ascii_lowercase();
            !matches!(v.as_str(), "0" | "false" | "no" | "")
        }
        None => true, // default on; set OMNIBUS_SECURE_COOKIES=0 only for non-localhost HTTP origins (e.g. LAN IPs)
    }
}

fn secure_cookies() -> bool {
    parse_secure_cookies(std::env::var("OMNIBUS_SECURE_COOKIES").ok().as_deref())
}

/// Build a session cookie. When `OMNIBUS_SECURE_COOKIES` is on (the
/// default) this emits a `__Host-omnibus_session` cookie; the
/// `__Host-` prefix is browser-enforced to require `Secure` + no
/// `Domain` + `Path=/`, all of which this helper already sets. On dev
/// origins with the flag flipped off it falls back to the plain
/// `omnibus_session` name (no `Secure`, same `Path=/`, still no
/// `Domain`) so loopback/LAN HTTP keeps working.
fn session_cookie(value: String, max_age_secs: i64) -> Cookie<'static> {
    let secure = secure_cookies();
    // Invariants required by the `__Host-` prefix when `secure` is true:
    // no Domain attribute, Path=/, Secure. We set all three unconditionally
    // here so a future edit can't break the prefix contract by accident.
    let mut c = Cookie::new(session_cookie_name(secure), value);
    c.set_http_only(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_secure(secure);
    c.set_max_age(time::Duration::seconds(max_age_secs));
    c
}

fn cleared_cookie() -> Cookie<'static> {
    let secure = secure_cookies();
    let mut c = Cookie::new(session_cookie_name(secure), "");
    c.set_http_only(true);
    c.set_same_site(SameSite::Lax);
    c.set_path("/");
    c.set_secure(secure);
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
    if let Some((token, _)) = extract_token(&headers) {
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
mod tests;
