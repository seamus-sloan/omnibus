//! `AuthUser` / `AdminUser` — axum extractors that resolve a live session
//! from either the `omnibus_session` cookie (web) or an
//! `Authorization: Bearer <token>` header (mobile), then hand the handler
//! a typed view of the authenticated user.

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use omnibus_db::auth::{self as auth_db, SessionAuthError, SessionKind};
use omnibus_shared::UserSummary;
use sqlx::SqlitePool;

/// Authenticated user resolved from either a session cookie or a bearer
/// token. Extractor returns `401 Unauthorized` on anything that isn't a
/// live session.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
    pub session_id: i64,
    pub session_kind: SessionKind,
}

impl AuthUser {
    pub fn summary(&self) -> UserSummary {
        UserSummary {
            id: self.id,
            username: self.username.clone(),
            is_admin: self.is_admin,
            can_upload: self.can_upload,
            can_edit: self.can_edit,
            can_download: self.can_download,
        }
    }
}

/// Admin-only wrapper. Extracting this rejects non-admin users with 403.
#[derive(Debug, Clone)]
pub struct AdminUser(pub AuthUser);

/// Pull a session token out of the request, preferring a `Bearer` header
/// over a cookie. Thin wrapper around [`auth_db::parse_session_token`] —
/// the pure-string parsing lives in `omnibus-db` so the rpc.rs server
/// functions can call the same logic without pulling axum-extra in.
pub(super) fn extract_token(headers: &HeaderMap) -> Option<(String, SessionKind)> {
    let authorization = headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let cookie_header = headers.get(header::COOKIE).and_then(|v| v.to_str().ok());
    auth_db::parse_session_token(authorization, cookie_header)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    tracing::error!(error = %e, "internal auth extractor error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

impl<S> FromRequestParts<S> for AuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    // The pool is read from `Extension<SqlitePool>` rather than from router
    // state so the same extractor works on the hand-written `/api/*` REST
    // router (which uses `with_state(AppState)`) and on the auto-mounted
    // Dioxus server-function router for `/api/rpc/*` (whose state type is
    // private). The top-level fullstack router in `server/src/main.rs`
    // installs `Extension(pool)` on every request.
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let pool = parts
            .extensions
            .get::<SqlitePool>()
            .cloned()
            .ok_or_else(|| internal("missing SqlitePool extension"))?;
        // The cookie/bearer → live-session contract (token precedence,
        // SHA-256 hashing, absolute + idle expiry, revocation) lives in
        // `auth_db::validate_session` so this extractor and the Dioxus
        // server-function path in `omnibus_frontend::rpc` cannot drift.
        let authorization = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok());
        let cookie_header = parts
            .headers
            .get(header::COOKIE)
            .and_then(|v| v.to_str().ok());
        match auth_db::validate_session(&pool, authorization, cookie_header).await {
            Ok((user, session)) => Ok(AuthUser {
                id: user.id,
                username: user.username,
                is_admin: user.is_admin,
                can_upload: user.can_upload,
                can_edit: user.can_edit,
                can_download: user.can_download,
                session_id: session.id,
                session_kind: session.kind,
            }),
            Err(SessionAuthError::Unauthenticated) => Err(unauthorized()),
            Err(SessionAuthError::Internal(e)) => Err(internal(e)),
        }
    }
}

impl<S> FromRequestParts<S> for AdminUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let user = AuthUser::from_request_parts(parts, state).await?;
        if !user.is_admin {
            return Err((StatusCode::FORBIDDEN, "admin required").into_response());
        }
        Ok(AdminUser(user))
    }
}

#[cfg(test)]
mod tests;
