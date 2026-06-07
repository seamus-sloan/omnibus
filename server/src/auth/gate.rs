//! `require_auth` — top-level middleware that gates `/api/*` routes behind a
//! live session.
//!
//! Applied in `server/src/main.rs`. The middleware fast-paths two classes of
//! request so SSR, assets, and the auth endpoints themselves keep working:
//!
//! * Anything that isn't a `/api/*` path (SSR HTML, WASM bundle, static
//!   assets, Dioxus client-side routes) — passes through untouched.
//! * Anything under `/api/auth/*` — these handle their own authentication
//!   (`/me` uses the [`AuthUser`] extractor; `/login`/`/register` deliberately
//!   don't require auth).
//! * `/api/_health` — unauthenticated liveness + fingerprint probe used by
//!   `scripts/dev-server-up.sh` to decide whether to reuse an existing
//!   omnibus server on the port. Cannot require auth or the probe couldn't
//!   run before a session exists.
//!
//! Everything else under `/api/*` — the REST routes (`/api/settings`,
//! `/api/library`, `/api/ebooks`, `/api/covers/{uuid}`) and the
//! Dioxus server-function endpoints (`/api/rpc/*`) — requires a valid session
//! or returns `401 Unauthorized`.
//!
//! This middleware does not set per-user request extensions: handlers that
//! need `AuthUser` should still declare it as an extractor. The middleware
//! only gates the *boundary*; the extractor provides the typed view.
//!
//! [`AuthUser`]: super::AuthUser

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use omnibus_db::auth as auth_db;

use super::extractor::extract_token;
use crate::backend::AppState;

pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/")
        || path == "/api/auth"
        || path.starts_with("/api/auth/")
        || path == "/api/_health"
    {
        return next.run(req).await;
    }
    let Some((token, _kind)) = extract_token(req.headers()) else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    match auth_db::lookup_session(state.pool(), &token).await {
        Ok(_) => next.run(req).await,
        Err(auth_db::AuthError::SessionNotFound) => {
            (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "require_auth: session lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

#[cfg(test)]
mod tests;
