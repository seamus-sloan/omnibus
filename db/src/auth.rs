//! Auth data layer: pure SQL + Argon2id hashing for users, devices, and
//! sessions. No axum types, no cookies — those belong to `server::auth`.
//! Covers password hashing/verify with PHC rotation, password-policy
//! validation, first-user-admin creation, per-account lockout, and
//! SHA-256-hashed session tokens with absolute + idle expiry.

mod avatars;
mod device;
mod login;
mod password;
mod session;
mod session_key;
mod token;
mod users;

pub use avatars::{delete_user_avatar, get_user_avatar, upsert_user_avatar, UserAvatar};
pub use device::{
    list_devices_for_user, register_device, validate_client_version, validate_device_name,
    LIST_DEVICES_LIMIT, MAX_CLIENT_VERSION_CHARS, MAX_DEVICES_PER_USER, MAX_DEVICE_NAME_CHARS,
};
pub use login::verify_login;
pub use password::{hash_password, validate_password, validate_username, verify_password};
pub use session::{
    create_session, lookup_session, prune_expired_sessions, revoke_all_sessions_for_user,
    revoke_all_sessions_for_user_except, revoke_session, validate_session, SessionAuthError,
};
pub use session_key::{get_session_key, load_or_create_session_key, put_session_key};
pub use token::{
    generate_token, hash_token, is_session_cookie_name, parse_session_token, SESSION_COOKIE_NAME,
    SESSION_COOKIE_NAME_HOST_PREFIXED,
};
pub use users::{
    admin_create_user, admin_set_password, change_password, create_user, delete_user,
    get_kindle_email, get_user_by_id, get_user_by_username, list_users, promote_to_admin,
    registration_enabled, set_display_name, set_kindle_email, set_registration_enabled,
    unlock_user, update_user_permissions, DISPLAY_NAME_MAX_LEN,
};

use sqlx::Row;

/// Auth-layer error space.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("invalid credentials")]
    InvalidCredentials,
    /// Input-rule rejection; wrapped string is the user-facing 400 body.
    #[error("{0}")]
    Validation(String),
    #[error("username is already taken")]
    UsernameTaken,
    #[error("user not found")]
    UserNotFound,
    #[error("cannot remove the last administrator")]
    LastAdmin,
    #[error("session not found or expired")]
    SessionNotFound,
    #[error("account is temporarily locked")]
    AccountLocked { until_unix: i64 },
    #[error("registration is disabled")]
    RegistrationDisabled,
    /// Opaque internal failure (DB I/O, etc). Carries no `sqlx::Error`
    /// so the storage backend doesn't leak through this crate's API.
    #[error("internal database error: {0}")]
    Internal(String),
    /// Argon2 hash/verify or CSPRNG byte-generation failure (maps to 500).
    #[error("{0}")]
    Crypto(String),
}

impl From<sqlx::Error> for AuthError {
    fn from(e: sqlx::Error) -> Self {
        AuthError::Internal(e.to_string())
    }
}

impl From<argon2::password_hash::Error> for AuthError {
    fn from(e: argon2::password_hash::Error) -> Self {
        AuthError::Crypto(format!("password hashing failed: {e}"))
    }
}

pub type AuthResult<T> = Result<T, AuthError>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
    /// Send-to-Kindle destination, or `None` when unconfigured.
    pub kindle_email: Option<String>,
    /// Presentation name shown to other users, or `None` to fall back to
    /// `username`. `username` stays the login and admin identity.
    pub display_name: Option<String>,
    /// Whether a `user_avatars` row exists — the avatar bytes themselves are
    /// only ever read by the serving handler.
    pub has_avatar: bool,
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

pub(crate) fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
        kindle_email: row.get("kindle_email"),
        display_name: row.get("display_name"),
        has_avatar: row.get::<i64, _>("has_avatar") != 0,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_error_from_sqlx_error_maps_to_internal_without_leaking_variant_type() {
        // Use any sqlx::Error variant — the conversion must collapse it
        // into the opaque `Internal` variant so server-side callers
        // cannot match on a `sqlx::Error` value (rule 02).
        let sqlx_err = sqlx::Error::RowNotFound;
        let auth_err: AuthError = sqlx_err.into();
        assert!(matches!(auth_err, AuthError::Internal(_)));
    }
}
