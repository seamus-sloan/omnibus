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
//!
//! Everything else under `/api/*` — the REST routes (`/api/value`,
//! `/api/settings`, `/api/library`, `/api/ebooks`, `/api/covers/{id}`) and the
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
    if !path.starts_with("/api/") || path == "/api/auth" || path.starts_with("/api/auth/") {
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
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, middleware::from_fn_with_state, routing::get, Router};
    use omnibus_db as db;
    use omnibus_db::auth::SessionKind;
    use tower::ServiceExt;

    async fn app() -> (Router, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        let state = AppState::new(pool.clone());
        let router = Router::new()
            .route("/api/value", get(|| async { "ok" }))
            .route("/api/auth/login", get(|| async { "login ok" }))
            .route("/", get(|| async { "home" }))
            .layer(from_fn_with_state(state, require_auth));
        (router, pool)
    }

    #[tokio::test]
    async fn non_api_passes_through_without_auth() {
        let (app, _pool) = app().await;
        let res = app
            .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn api_auth_passes_through_without_auth() {
        let (app, _pool) = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/login")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gated_api_without_auth_is_401() {
        let (app, _pool) = app().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/value")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn gated_api_with_bearer_passes() {
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
                    .uri("/api/value")
                    .header(
                        axum::http::header::AUTHORIZATION,
                        format!("Bearer {}", issued.raw_token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gated_api_with_cookie_passes() {
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
                    .uri("/api/value")
                    .header(
                        axum::http::header::COOKIE,
                        format!("{}={}", super::super::SESSION_COOKIE, issued.raw_token),
                    )
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn gated_api_with_revoked_session_is_401() {
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
        db::auth::revoke_session(&pool, issued.session.id)
            .await
            .unwrap();
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/value")
                    .header(
                        axum::http::header::AUTHORIZATION,
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
