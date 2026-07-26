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
    pub kindle_email: Option<String>,
    pub session_id: i64,
    pub session_kind: SessionKind,
}

impl AuthUser {
    /// Project this extractor result to the wire-shape `UserSummary` returned by `/api/auth/me`.
    pub fn summary(&self) -> UserSummary {
        UserSummary {
            id: self.id,
            username: self.username.clone(),
            is_admin: self.is_admin,
            can_upload: self.can_upload,
            can_edit: self.can_edit,
            can_download: self.can_download,
            kindle_email: self.kindle_email.clone(),
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

/// Pull the session token out of a `?token=<value>` query parameter.
/// Session tokens are URL-safe base64 (no padding), so no percent-decoding
/// is needed. Used by [`MediaAuthUser`] and the `require_auth` gate for the
/// media read paths — see [`MediaAuthUser`]'s docs for why.
pub(super) fn query_token(query: Option<&str>) -> Option<String> {
    query?.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=')?;
        (name == "token" && !value.is_empty()).then(|| value.to_string())
    })
}

/// Resolve a live session from a request's cookie / bearer header. When
/// `allow_query_token` is set and no `Authorization` header is present, a
/// `?token=` query parameter is accepted as a bearer fallback.
async fn resolve_auth_user(
    parts: &mut Parts,
    allow_query_token: bool,
) -> Result<AuthUser, Response> {
    let pool = parts
        .extensions
        .get::<SqlitePool>()
        .cloned()
        .ok_or_else(|| internal("missing SqlitePool extension"))?;
    // The cookie/bearer → live-session contract (token precedence,
    // SHA-256 hashing, absolute + idle expiry, revocation) lives in
    // `auth_db::validate_session` so this extractor and the Dioxus
    // server-function path in `omnibus_frontend::rpc` cannot drift.
    let header_auth = parts
        .headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok());
    let cookie_header = parts
        .headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok());
    // The query token is a strict fallback: it fills in only when neither a
    // bearer header nor a session cookie is present, so a URL `?token=` can
    // never override an active cookie/bearer session.
    let has_session_creds = auth_db::parse_session_token(header_auth, cookie_header).is_some();
    let query_auth = (allow_query_token && !has_session_creds)
        .then(|| query_token(parts.uri.query()).map(|t| format!("Bearer {t}")))
        .flatten();
    let authorization = header_auth.or(query_auth.as_deref());
    match auth_db::validate_session(&pool, authorization, cookie_header).await {
        Ok((user, session)) => Ok(build_auth_user(user, session)),
        Err(SessionAuthError::Unauthenticated) => Err(unauthorized()),
        Err(SessionAuthError::Internal(e)) => Err(internal(e)),
    }
}

/// Assemble the extractor's `AuthUser` from a validated user + session pair.
/// Shared by the header/cookie path ([`resolve_auth_user`]) and the raw-token
/// path ([`resolve_session_token`]) so the projection never drifts.
fn build_auth_user(user: auth_db::User, session: auth_db::Session) -> AuthUser {
    AuthUser {
        id: user.id,
        username: user.username,
        is_admin: user.is_admin,
        can_upload: user.can_upload,
        can_edit: user.can_edit,
        can_download: user.can_download,
        kindle_email: user.kindle_email,
        session_id: session.id,
        session_kind: session.kind,
    }
}

/// Resolve a live session from a **raw** session-token string (no `Bearer`
/// framing, no header/cookie/query). Used by the Kobo path-token extractor,
/// which carries the token in the URL path — a channel none of the standard
/// extractors read. Returns the same [`AuthUser`] the header path produces.
pub async fn resolve_session_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<AuthUser, SessionAuthError> {
    let authorization = format!("Bearer {token}");
    let (user, session) = auth_db::validate_session(pool, Some(&authorization), None).await?;
    Ok(build_auth_user(user, session))
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
        resolve_auth_user(parts, false).await
    }
}

/// Authenticated user for **media read endpoints** (`/api/covers`,
/// `/api/thumbs`, `/api/authors/{id}/photo`, and direct-play audiobook
/// `/api/audiobooks/{uuid}/parts/{ordinal}` streams). Identical to
/// [`AuthUser`] except it also accepts the session token from a `?token=`
/// query parameter.
///
/// These URLs are wired straight into an `<img src>` / `<audio src>` in the
/// mobile WebView, which fetches them without the native `reqwest` client's
/// `Authorization` header (and without a session cookie, since mobile auth is
/// bearer-only) — so a header/cookie-only gate 401s every cover and audio
/// stream on mobile. The query token is the only auth such a fetch can carry.
/// Confined to these reads; the full API stays header/cookie-only.
#[derive(Debug, Clone)]
pub struct MediaAuthUser(pub AuthUser);

impl<S> FromRequestParts<S> for MediaAuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        resolve_auth_user(parts, true).await.map(MediaAuthUser)
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
