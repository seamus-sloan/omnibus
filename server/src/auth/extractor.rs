//! `AuthUser` / `AdminUser` — axum extractors that resolve a live session
//! from either the `omnibus_session` cookie (web) or an
//! `Authorization: Bearer <token>` header (mobile), then hand the handler
//! a typed view of the authenticated user.

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;
use omnibus_db::auth::{self as auth_db, AuthError, SessionKind};
use omnibus_shared::UserSummary;
use sqlx::SqlitePool;

use super::SESSION_COOKIE;

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
/// over a cookie. Returns `None` when neither source has a non-empty token.
pub(super) fn extract_token(headers: &HeaderMap, jar: &CookieJar) -> Option<(String, SessionKind)> {
    if let Some(value) = headers.get(header::AUTHORIZATION) {
        if let Ok(s) = value.to_str() {
            if let Some(rest) = s.strip_prefix("Bearer ") {
                let token = rest.trim().to_string();
                if !token.is_empty() {
                    return Some((token, SessionKind::Bearer));
                }
            }
        }
    }
    if let Some(cookie) = jar.get(SESSION_COOKIE) {
        let token = cookie.value().to_string();
        if !token.is_empty() {
            return Some((token, SessionKind::Cookie));
        }
    }
    None
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
        let jar = CookieJar::from_headers(&parts.headers);
        let Some((token, _kind)) = extract_token(&parts.headers, &jar) else {
            return Err(unauthorized());
        };
        match auth_db::lookup_session(&pool, &token).await {
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
            Err(AuthError::SessionNotFound) => Err(unauthorized()),
            Err(e) => Err(internal(e)),
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
mod tests {
    use super::*;
    use crate::auth::handlers::auth_router;
    use crate::backend::AppState;
    use axum::{body::Body, http::Request};
    use omnibus_db as db;
    use tower::ServiceExt;

    async fn app() -> (axum::Router, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        let router = auth_router(AppState::new(pool.clone()));
        (router, pool)
    }

    #[tokio::test]
    async fn me_without_auth_is_401() {
        let (app, _pool) = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn me_with_bearer_returns_user() {
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
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let u: UserSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(u.username, "alice");
    }

    #[tokio::test]
    async fn me_with_cookie_returns_user() {
        let (app, pool) = app().await;
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        let user = db::auth::get_user_by_username(&pool, "alice")
            .await
            .unwrap()
            .unwrap();
        let issued = db::auth::create_session(&pool, user.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .header(
                        header::COOKIE,
                        format!("{}={}", SESSION_COOKIE, issued.raw_token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
