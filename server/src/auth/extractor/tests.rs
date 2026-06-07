//! Tests for AuthUser and AdminUser extractors.
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
                    format!("{}={}", crate::auth::SESSION_COOKIE, issued.raw_token),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
