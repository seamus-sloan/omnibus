//! Auth data layer (F0.3).
//!
//! Pure SQL + hashing. No axum types, no cookies — those belong to
//! `server::auth`. This module owns:
//!
//! * Argon2id password hashing + verification + PHC rotation on verify.
//! * Password-policy validation (length + common-password reject-list).
//! * Race-free first-user-admin creation (BEGIN IMMEDIATE).
//! * Timing-safe login with per-account lockout + failure counter.
//! * Session creation: raw 256-bit token returned once, SHA-256 hash stored.
//! * Session lookup: exact SHA-256 hash match against the stored value
//!   (the raw token is never persisted), with absolute expiry
//!   (`expires_at`, set at create time from the caller's TTL — 30 days for
//!   cookies, 90 days for bearer tokens) and idle expiry
//!   (`SESSION_IDLE_TIMEOUT_SECS`, 7 days since `last_used_at`).
//! * Device registration + listing.
//! * `OMNIBUS_INITIAL_ADMIN` recovery hook (`promote_to_admin`).
//! * Session-key secret load/create in `secrets`.
//!
//! Schema lives in `migrations/0004_auth.sql`. See
//! `docs/roadmap/0-3-auth.md` for the security rationale behind every
//! design decision here.

mod device;
mod login;
mod password;
mod session;
mod session_key;
mod token;
mod users;

pub use device::{list_devices_for_user, register_device};
pub use login::verify_login;
pub use password::{hash_password, validate_password, verify_password};
pub use session::{
    create_session, lookup_session, prune_expired_sessions, revoke_all_sessions_for_user,
    revoke_session, validate_session, SessionAuthError,
};
pub use session_key::{get_session_key, load_or_create_session_key, put_session_key};
pub use token::{generate_token, hash_token, parse_session_token, SESSION_COOKIE_NAME};
pub use users::{
    create_user, get_user_by_id, get_user_by_username, promote_to_admin, registration_enabled,
    set_registration_enabled,
};

use sqlx::Row;

// -----------------------------------------------------------------------------
// Errors
// -----------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    #[error("account is temporarily locked")]
    AccountLocked { until_unix: i64 },
    #[error("username is already taken")]
    UsernameTaken,
    #[error("password is too short (min {min} chars)")]
    PasswordTooShort { min: usize },
    #[error("password is too long (max {max} chars)")]
    PasswordTooLong { max: usize },
    #[error("password is on the common-passwords reject list")]
    PasswordCommon,
    #[error("registration is disabled")]
    RegistrationDisabled,
    #[error("session not found or expired")]
    SessionNotFound,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("password hashing failed: {0}")]
    Hash(String),
}

impl From<argon2::password_hash::Error> for AuthError {
    fn from(e: argon2::password_hash::Error) -> Self {
        AuthError::Hash(e.to_string())
    }
}

pub type AuthResult<T> = Result<T, AuthError>;

// -----------------------------------------------------------------------------
// Domain types
// -----------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    pub id: i64,
    pub user_id: i64,
    pub device_id: Option<i64>,
    pub kind: SessionKind,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionKind {
    Cookie,
    Bearer,
}

impl SessionKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            SessionKind::Cookie => "cookie",
            SessionKind::Bearer => "bearer",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub client_kind: String,
    pub client_version: Option<String>,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// Returned from `create_session`. Callers must send `raw_token` to the
/// client exactly once — the server only keeps `SHA-256(raw_token)`.
pub struct NewSession {
    pub session: Session,
    pub raw_token: String,
}

// -----------------------------------------------------------------------------
// Cross-cutting helpers
// -----------------------------------------------------------------------------

pub(crate) fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
    }
}

pub(crate) fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
pub(crate) mod test_support {
    use crate::pool::init_db;
    use sqlx::SqlitePool;

    pub async fn pool() -> SqlitePool {
        init_db("sqlite::memory:").await.expect("pool init")
    }
}
