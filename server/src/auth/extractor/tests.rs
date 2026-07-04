//! Tests for AuthUser, AdminUser, and MediaAuthUser extractors.
use super::*;
use crate::auth::handlers::auth_router;
use crate::backend::AppState;
use axum::{body::Body, http::Request, routing::get, Extension, Router};
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

#[test]
fn query_token_finds_the_token_param() {
    assert_eq!(query_token(Some("token=abc")).as_deref(), Some("abc"));
    assert_eq!(
        query_token(Some("size=md&token=abc&x=1")).as_deref(),
        Some("abc")
    );
}

#[test]
fn query_token_is_none_when_absent_empty_or_no_query() {
    assert_eq!(query_token(None), None);
    assert_eq!(query_token(Some("size=md")), None);
    assert_eq!(query_token(Some("token=")), None);
    // A key that merely ends in `token` must not match.
    assert_eq!(query_token(Some("xtoken=abc")), None);
}

/// Minimal router whose single route is gated on [`MediaAuthUser`], so the
/// query-token path can be exercised without a real cover on disk.
async fn media_app() -> (Router, sqlx::SqlitePool) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    async fn probe(_user: MediaAuthUser) -> StatusCode {
        StatusCode::OK
    }
    let router = Router::new()
        .route("/media", get(probe))
        .layer(Extension(pool.clone()));
    (router, pool)
}

async fn seed_bearer_token(pool: &sqlx::SqlitePool) -> String {
    db::auth::create_user(pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    let user = db::auth::get_user_by_username(pool, "alice")
        .await
        .unwrap()
        .unwrap();
    db::auth::create_session(pool, user.id, None, SessionKind::Bearer, 3600)
        .await
        .unwrap()
        .raw_token
}

#[tokio::test]
async fn media_auth_accepts_a_valid_query_token() {
    let (app, pool) = media_app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/media?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_auth_still_accepts_a_bearer_header() {
    let (app, pool) = media_app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/media")
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_auth_rejects_missing_or_invalid_query_token() {
    let (app, pool) = media_app().await;
    seed_bearer_token(&pool).await;
    for uri in ["/media", "/media?token=not-a-real-token"] {
        let res = app
            .clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "uri = {uri}");
    }
}
