//! Tests for the rpc-side `AuthUser`/`AdminUser` extractors — the sole
//! 401/403 gate for every `/api/rpc/*` server function reachable from the
//! web UI. Mirrors `server/src/auth/extractor/tests.rs`'s test names/shapes
//! rather than reusing it directly (see `server_auth`'s module doc for why
//! it's a deliberate duplicate).
use dioxus::fullstack::axum::extract::FromRequestParts;
use dioxus::fullstack::axum::http::request::Parts;
use dioxus::fullstack::axum::http::{header, HeaderName, Request, StatusCode};
use omnibus_db::auth::{self as auth_db, SessionKind, SESSION_COOKIE_NAME};
use sqlx::SqlitePool;

use super::{AdminUser, AuthUser};

async fn pool() -> SqlitePool {
    omnibus_db::init_db("sqlite::memory:").await.unwrap()
}

/// Build request `Parts` carrying `pool` as an extension plus `headers` —
/// the shape the extractors read straight off a request, without a full
/// HTTP round trip through a router.
fn parts_with(pool: SqlitePool, headers: &[(HeaderName, &str)]) -> Parts {
    let mut builder = Request::builder().uri("/api/rpc/probe");
    for (name, value) in headers {
        builder = builder.header(name.clone(), *value);
    }
    let mut parts = builder.body(()).unwrap().into_parts().0;
    parts.extensions.insert(pool);
    parts
}

/// Seed a user + live session of `kind`, returning the raw session token.
/// Re-enables registration first — creating the first user disables it, so
/// seeding a second user (e.g. a non-admin alongside the auto-admin first
/// user) would otherwise fail.
async fn seed_session(pool: &SqlitePool, username: &str, kind: SessionKind) -> String {
    auth_db::set_registration_enabled(pool, true).await.unwrap();
    auth_db::create_user(pool, username, "correct horse battery staple")
        .await
        .unwrap();
    let user = auth_db::get_user_by_username(pool, username)
        .await
        .unwrap()
        .unwrap();
    auth_db::create_session(pool, user.id, None, kind, 3600)
        .await
        .unwrap()
        .raw_token
}

#[tokio::test]
async fn auth_user_from_request_parts_is_rejected_when_no_session() {
    let p = pool().await;
    let mut parts = parts_with(p, &[]);
    let res = AuthUser::from_request_parts(&mut parts, &())
        .await
        .unwrap_err();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_user_from_request_parts_returns_user_for_valid_session_cookie() {
    let p = pool().await;
    let token = seed_session(&p, "alice", SessionKind::Cookie).await;
    let user = auth_db::get_user_by_username(&p, "alice")
        .await
        .unwrap()
        .unwrap();
    let cookie = format!("{SESSION_COOKIE_NAME}={token}");
    let mut parts = parts_with(p, &[(header::COOKIE, &cookie)]);
    let auth_user = AuthUser::from_request_parts(&mut parts, &()).await.unwrap();
    assert_eq!(auth_user.id, user.id);
    assert!(auth_user.is_admin);
}

#[tokio::test]
async fn auth_user_from_request_parts_returns_user_for_valid_bearer_token() {
    let p = pool().await;
    let token = seed_session(&p, "alice", SessionKind::Bearer).await;
    let user = auth_db::get_user_by_username(&p, "alice")
        .await
        .unwrap()
        .unwrap();
    let bearer = format!("Bearer {token}");
    let mut parts = parts_with(p, &[(header::AUTHORIZATION, &bearer)]);
    let auth_user = AuthUser::from_request_parts(&mut parts, &()).await.unwrap();
    assert_eq!(auth_user.id, user.id);
    assert!(auth_user.is_admin);
}

#[tokio::test]
async fn admin_user_from_request_parts_is_rejected_when_user_is_not_admin() {
    let p = pool().await;
    // alice is the first user (auto-admin); bob is created second and is
    // never granted admin.
    seed_session(&p, "alice", SessionKind::Cookie).await;
    let token = seed_session(&p, "bob", SessionKind::Bearer).await;
    let bearer = format!("Bearer {token}");
    let mut parts = parts_with(p, &[(header::AUTHORIZATION, &bearer)]);
    let res = AdminUser::from_request_parts(&mut parts, &())
        .await
        .unwrap_err();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn admin_user_from_request_parts_returns_admin_when_user_is_admin() {
    let p = pool().await;
    let token = seed_session(&p, "alice", SessionKind::Bearer).await;
    let user = auth_db::get_user_by_username(&p, "alice")
        .await
        .unwrap()
        .unwrap();
    let bearer = format!("Bearer {token}");
    let mut parts = parts_with(p, &[(header::AUTHORIZATION, &bearer)]);
    let admin_user = AdminUser::from_request_parts(&mut parts, &())
        .await
        .unwrap();
    assert_eq!(admin_user.0.id, user.id);
    assert!(admin_user.0.is_admin);
}
