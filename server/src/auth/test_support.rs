//! Helpers for bootstrapping authenticated sessions in handler integration
//! tests. Lets each test attach a real bearer token (or cookie) in one line
//! instead of repeating the create-user → create-session dance.
//!
//! User creation goes through raw SQL rather than [`db::auth::create_user`]
//! because the production helper auto-promotes the first user to admin and
//! flips `registration_enabled` off afterward. Tests want explicit role
//! assignment, not the registration policy.

use omnibus_db::{
    self as db,
    auth::{NewSession, SessionKind, User},
};
use sqlx::SqlitePool;

use super::{BEARER_TTL_SECS, COOKIE_TTL_SECS, SESSION_COOKIE};

/// Insert a user with the given `is_admin` flag, bypassing the registration
/// gate and first-user auto-promote logic. The password hash is a sentinel
/// that no `verify_password` call will accept — these test users only log
/// in via direct session minting.
async fn insert_user(pool: &SqlitePool, username: &str, is_admin: bool) -> User {
    let admin_flag = if is_admin { 1i64 } else { 0 };
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!test-no-password', ?, ?, ?, 1)
         RETURNING id",
    )
    .bind(username)
    .bind(admin_flag)
    .bind(admin_flag)
    .bind(admin_flag)
    .fetch_one(pool)
    .await
    .expect("insert user");
    User {
        id,
        username: username.to_string(),
        is_admin,
        can_upload: is_admin,
        can_edit: is_admin,
        can_download: true,
    }
}

/// Create a non-admin user.
pub async fn create_user(pool: &SqlitePool, username: &str) -> User {
    insert_user(pool, username, false).await
}

/// Create an admin user.
pub async fn create_admin(pool: &SqlitePool, username: &str) -> User {
    insert_user(pool, username, true).await
}

/// Issue a bearer session for `user_id`. Returns the raw token (no `Bearer `
/// prefix) — call sites format it as `format!("Bearer {token}")`.
pub async fn bearer_token(pool: &SqlitePool, user_id: i64) -> String {
    let issued: NewSession =
        db::auth::create_session(pool, user_id, None, SessionKind::Bearer, BEARER_TTL_SECS)
            .await
            .expect("create_session should succeed");
    issued.raw_token
}

/// Issue a cookie session for `user_id`. Returns the full `name=value` pair
/// suitable for the `Cookie` request header.
pub async fn cookie_value(pool: &SqlitePool, user_id: i64) -> String {
    let issued: NewSession =
        db::auth::create_session(pool, user_id, None, SessionKind::Cookie, COOKIE_TTL_SECS)
            .await
            .expect("create_session should succeed");
    format!("{}={}", SESSION_COOKIE, issued.raw_token)
}
