//! Tests for the auth gate middleware.
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
        .route("/api/thumbs/{uuid}/{size}", get(|| async { "thumb ok" }))
        .route(
            "/api/audiobooks/{uuid}/parts/{ordinal}",
            get(|| async { "part ok" }),
        )
        .route("/api/ebooks/{uuid}/file", get(|| async { "file ok" }))
        .route("/api/auth/login", get(|| async { "login ok" }))
        .route("/api/_health", get(|| async { "health ok" }))
        .route("/", get(|| async { "home" }))
        .layer(from_fn_with_state(state, require_auth));
    (router, pool)
}

/// Seed a user + live bearer session, returning the raw token.
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
async fn api_health_passes_through_without_auth() {
    let (app, _pool) = app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/_health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
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
async fn bare_api_auth_path_passes_gate_then_404s() {
    // Locks in issue #279: the `path == "/api/auth"` allow-list entry
    // is intentional (some HTTP clients normalize trailing slashes
    // during redirect following), but no real handler is mounted at
    // the bare `/api/auth` root. If a future change accidentally
    // mounts one — e.g. a status endpoint on the auth sub-router —
    // that handler would be unauthenticated. This test catches that
    // class of change by asserting the bare path still 404s and never
    // 200s, so the gate's pass-through stays free of routable
    // surface area.
    let (app, _pool) = app().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/auth")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
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

#[tokio::test]
async fn media_get_with_query_token_passes() {
    let (app, pool) = app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/thumbs/some-uuid/md?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn media_get_with_invalid_query_token_is_401() {
    let (app, pool) = app().await;
    seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/thumbs/some-uuid/md?token=not-a-real-token")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn audiobook_part_get_with_query_token_passes() {
    // The mobile WebView's `<audio src>` can only carry the session as
    // `?token=`; the direct-play part stream must accept it like covers do.
    let (app, pool) = app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/audiobooks/some-uuid/parts/0?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn audiobook_part_get_without_auth_is_401() {
    let (app, pool) = app().await;
    seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/audiobooks/some-uuid/parts/0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn ebook_file_get_with_query_token_passes() {
    // epub.js fetches the EPUB from the mobile WebView with the session as
    // `?token=`; the `/file` stream must accept it like the audiobook parts do.
    let (app, pool) = app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/ebooks/some-uuid/file?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn ebook_file_get_without_auth_is_401() {
    let (app, pool) = app().await;
    seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/ebooks/some-uuid/file")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn non_media_path_ignores_query_token() {
    // The `?token=` fallback is confined to media read paths; a valid token
    // in the query must NOT unlock a normal gated route.
    let (app, pool) = app().await;
    let token = seed_bearer_token(&pool).await;
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/api/value?token={token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
