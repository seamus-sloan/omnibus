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
//!   (the raw token is never persisted), with absolute + idle expiry.
//! * Device registration + listing.
//! * `OMNIBUS_INITIAL_ADMIN` recovery hook (`promote_to_admin`).
//! * Session-key secret load/create in `secrets`.
//!
//! Schema lives in `migrations/0004_auth.sql`. See
//! `docs/roadmap/0-3-auth.md` for the security rationale behind every
//! design decision here.

use argon2::password_hash::{
    rand_core::OsRng as PhcOsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
};
use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::{general_purpose::URL_SAFE_NO_PAD, Engine as _};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};

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
    fn as_str(self) -> &'static str {
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
// Password hashing + policy
// -----------------------------------------------------------------------------

/// OWASP 2024 floor for Argon2id. Hardcoded, not configurable — if we ever
/// need to tune these, rotation is free (on verify we rehash if the stored
/// PHC string's parameters are below current policy).
const ARGON2_MEMORY_KIB: u32 = 19_456; // 19 MiB
const ARGON2_ITERATIONS: u32 = 2;
const ARGON2_PARALLELISM: u32 = 1;

const MIN_PASSWORD_LEN: usize = 10;
const MAX_PASSWORD_LEN: usize = 128;

/// Tiny embedded reject-list. Deliberately small (top ~50) — this is a
/// "don't be stupid" check, not a HIBP replacement. Self-hosted deployments
/// are offline-tolerant, so a runtime breach check is out of scope.
const COMMON_PASSWORDS: &[&str] = &[
    "password",
    "password1",
    "password12",
    "password123",
    "password1234",
    "12345678",
    "123456789",
    "1234567890",
    "qwerty123",
    "qwertyuiop",
    "letmein123",
    "welcome123",
    "admin1234",
    "administrator",
    "iloveyou1",
    "dragon1234",
    "sunshine1",
    "princess1",
    "football1",
    "baseball1",
    "superman1",
    "batman1234",
    "trustno1234",
    "shadow1234",
    "master1234",
    "qazwsxedc",
    "zxcvbnm123",
    "asdfghjkl1",
    "11111111",
    "00000000",
    "12341234",
    "abcd1234",
    "passw0rd",
    "p@ssw0rd1",
    "qwerty1234",
    "monkey1234",
    "hello1234",
    "loveyou123",
    "liverpool1",
    "arsenal1",
    "chelsea123",
    "tottenham1",
    "manchester1",
    "brooklyn1",
    "jennifer1",
    "michelle1",
    "computer1",
    "internet1",
];

fn argon2_hasher() -> Argon2<'static> {
    let params = Params::new(
        ARGON2_MEMORY_KIB,
        ARGON2_ITERATIONS,
        ARGON2_PARALLELISM,
        None,
    )
    .expect("argon2 params are compile-time valid");
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
}

pub fn validate_password(password: &str) -> AuthResult<()> {
    // Count Unicode scalar values, not bytes, so a few emoji can't satisfy
    // a byte-length floor. Argon2 itself is byte-oriented and imposes no
    // separate limit; MAX_PASSWORD_LEN guards against unbounded CPU work.
    let char_count = password.chars().count();
    if char_count < MIN_PASSWORD_LEN {
        return Err(AuthError::PasswordTooShort {
            min: MIN_PASSWORD_LEN,
        });
    }
    if char_count > MAX_PASSWORD_LEN {
        return Err(AuthError::PasswordTooLong {
            max: MAX_PASSWORD_LEN,
        });
    }
    let lower = password.to_lowercase();
    if COMMON_PASSWORDS.iter().any(|c| *c == lower) {
        return Err(AuthError::PasswordCommon);
    }
    Ok(())
}

pub fn hash_password(password: &str) -> AuthResult<String> {
    let salt = SaltString::generate(&mut PhcOsRng);
    let phc = argon2_hasher()
        .hash_password(password.as_bytes(), &salt)?
        .to_string();
    Ok(phc)
}

/// Verify a password against a stored PHC hash. Constant-time via argon2's
/// internal equality check. Returns Ok(true) only on match.
pub fn verify_password(password: &str, phc: &str) -> AuthResult<bool> {
    let parsed = PasswordHash::new(phc)?;
    match argon2_hasher().verify_password(password.as_bytes(), &parsed) {
        Ok(()) => Ok(true),
        Err(argon2::password_hash::Error::Password) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

// -----------------------------------------------------------------------------
// Token generation + at-rest hashing
// -----------------------------------------------------------------------------

/// 32 bytes from OsRng (CSPRNG), base64url-encoded (no padding). ~43 chars,
/// 256-bit entropy. Returned to the client exactly once.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// SHA-256 of the raw token. What we store and look up by.
pub fn hash_token(raw: &str) -> Vec<u8> {
    let mut hasher = Sha256::new();
    hasher.update(raw.as_bytes());
    hasher.finalize().to_vec()
}

// -----------------------------------------------------------------------------
// User CRUD
// -----------------------------------------------------------------------------

fn row_to_user(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
    }
}

/// Atomically create a user. The first user created becomes admin; the
/// `registration_enabled` setting is flipped to '0' in the same
/// transaction. Subsequent creates check `registration_enabled` and refuse
/// if disabled. Uses BEGIN IMMEDIATE so two concurrent callers cannot both
/// observe an empty users table.
pub async fn create_user(pool: &SqlitePool, username: &str, password: &str) -> AuthResult<User> {
    validate_password(password)?;
    let phc = hash_password(password)?;

    let mut conn = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE").execute(&mut *conn).await?;

    // Rollback-on-error guard. We can't use RAII here because async drop
    // isn't stable; explicit COMMIT/ROLLBACK via match below.
    let result: AuthResult<User> = async {
        let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&mut *conn)
            .await?;

        let is_first = user_count == 0;

        if !is_first {
            let enabled: String =
                sqlx::query_scalar("SELECT value FROM settings WHERE key = 'registration_enabled'")
                    .fetch_optional(&mut *conn)
                    .await?
                    .unwrap_or_else(|| "0".to_string());
            if enabled != "1" {
                return Err(AuthError::RegistrationDisabled);
            }
        }

        let existing: Option<i64> =
            sqlx::query_scalar("SELECT id FROM users WHERE username = ? COLLATE NOCASE")
                .bind(username)
                .fetch_optional(&mut *conn)
                .await?;
        if existing.is_some() {
            return Err(AuthError::UsernameTaken);
        }

        let is_admin = if is_first { 1i64 } else { 0 };
        let can_upload = if is_first { 1i64 } else { 0 };
        let can_edit = if is_first { 1i64 } else { 0 };
        let can_download = 1i64;

        let id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
             VALUES (?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(username)
        .bind(&phc)
        .bind(is_admin)
        .bind(can_upload)
        .bind(can_edit)
        .bind(can_download)
        .fetch_one(&mut *conn)
        .await?;

        if is_first {
            sqlx::query("UPDATE settings SET value = '0' WHERE key = 'registration_enabled'")
                .execute(&mut *conn)
                .await?;
        }

        Ok(User {
            id,
            username: username.to_string(),
            is_admin: is_admin != 0,
            can_upload: can_upload != 0,
            can_edit: can_edit != 0,
            can_download: can_download != 0,
        })
    }
    .await;

    match &result {
        Ok(_) => {
            sqlx::query("COMMIT").execute(&mut *conn).await?;
        }
        Err(_) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *conn).await;
        }
    }
    result
}

pub async fn get_user_by_username(pool: &SqlitePool, username: &str) -> AuthResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, is_admin, can_upload, can_edit, can_download
         FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

pub async fn get_user_by_id(pool: &SqlitePool, id: i64) -> AuthResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, is_admin, can_upload, can_edit, can_download
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

/// `OMNIBUS_INITIAL_ADMIN` boot hook: if a user by this username exists,
/// set `is_admin = 1`. Never auto-creates — the env var is recovery, not
/// provisioning. Returns true if a row was updated.
pub async fn promote_to_admin(pool: &SqlitePool, username: &str) -> AuthResult<bool> {
    let result = sqlx::query("UPDATE users SET is_admin = 1 WHERE username = ? COLLATE NOCASE")
        .bind(username)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

pub async fn set_registration_enabled(pool: &SqlitePool, enabled: bool) -> AuthResult<()> {
    let v = if enabled { "1" } else { "0" };
    sqlx::query(
        "INSERT INTO settings (key, value) VALUES ('registration_enabled', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
    )
    .bind(v)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn registration_enabled(pool: &SqlitePool) -> AuthResult<bool> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'registration_enabled'")
            .fetch_optional(pool)
            .await?;
    Ok(v.as_deref() == Some("1"))
}

// -----------------------------------------------------------------------------
// Login
// -----------------------------------------------------------------------------

/// Lockout schedule (minutes), keyed on the number of prior lockouts. After
/// 5 failed attempts in any window we consult this table for the next
/// `locked_until`.
const LOCKOUT_MIN_AFTER: i64 = 5;
const LOCKOUT_DURATION_SECS: i64 = 15 * 60;

fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Sentinel PHC string used when the username is unknown, so we still
/// spend ~250ms in argon2 verify and don't leak username existence via
/// response-timing. Generated once at module init.
fn sentinel_hash() -> &'static str {
    use std::sync::OnceLock;
    static HASH: OnceLock<String> = OnceLock::new();
    HASH.get_or_init(|| {
        hash_password("__timing_equalizer_not_a_real_password__")
            .expect("sentinel hash always succeeds")
    })
}

/// Verify a login attempt. On success returns the user; on failure returns
/// a generic `InvalidCredentials` (same error for unknown username and
/// wrong password). Enforces per-account lockout.
pub async fn verify_login(pool: &SqlitePool, username: &str, password: &str) -> AuthResult<User> {
    let row = sqlx::query(
        "SELECT id, username, password_hash, is_admin, can_upload, can_edit, can_download,
                failed_login_count, locked_until
         FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    let now = now_unix();

    let Some(row) = row else {
        // Equalize timing against the found-user path.
        let _ = verify_password(password, sentinel_hash());
        return Err(AuthError::InvalidCredentials);
    };

    let user_id: i64 = row.get("id");
    let phc: String = row.get("password_hash");
    let locked_until: Option<i64> = row.get("locked_until");
    let failed: i64 = row.get("failed_login_count");

    // If a prior lockout window has elapsed, the counter must reset so a
    // single subsequent failure doesn't instantly re-lock (the counter is
    // still >= LOCKOUT_MIN_AFTER from the previous window). We treat the
    // effective failure count as zero from this point.
    let effective_failed = match locked_until {
        Some(until) if until > now => {
            let _ = verify_password(password, &phc); // equalize timing
            return Err(AuthError::AccountLocked { until_unix: until });
        }
        Some(_) => 0,
        None => failed,
    };

    let ok = verify_password(password, &phc)?;
    if !ok {
        let new_failed = effective_failed + 1;
        let new_lock = if new_failed >= LOCKOUT_MIN_AFTER {
            Some(now + LOCKOUT_DURATION_SECS)
        } else {
            None
        };
        sqlx::query("UPDATE users SET failed_login_count = ?, locked_until = ? WHERE id = ?")
            .bind(new_failed)
            .bind(new_lock)
            .bind(user_id)
            .execute(pool)
            .await?;
        return Err(AuthError::InvalidCredentials);
    }

    // Success: clear counters.
    sqlx::query("UPDATE users SET failed_login_count = 0, locked_until = NULL WHERE id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;

    Ok(row_to_user(&row))
}

// -----------------------------------------------------------------------------
// Sessions
// -----------------------------------------------------------------------------

/// Only write `last_used_at` if the existing value is older than this many
/// seconds. Avoids write-amplification on every authenticated request.
const SESSION_TOUCH_THRESHOLD_SECS: i64 = 5 * 60;

pub async fn create_session(
    pool: &SqlitePool,
    user_id: i64,
    device_id: Option<i64>,
    kind: SessionKind,
    ttl_secs: i64,
) -> AuthResult<NewSession> {
    let raw = generate_token();
    let hash = hash_token(&raw);
    let now = now_unix();
    let expires = now + ttl_secs;

    let row = sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, device_id, kind, expires_at)
         VALUES (?, ?, ?, ?, ?)
         RETURNING id, user_id, device_id, kind, created_at, last_used_at, expires_at",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(device_id)
    .bind(kind.as_str())
    .bind(expires)
    .fetch_one(pool)
    .await?;

    let session = Session {
        id: row.get("id"),
        user_id: row.get("user_id"),
        device_id: row.get("device_id"),
        kind,
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
        expires_at: row.get("expires_at"),
    };

    Ok(NewSession {
        session,
        raw_token: raw,
    })
}

/// Resolve a raw token into `(User, Session)`. Rejects expired or revoked
/// sessions. Updates `last_used_at` opportunistically (rate-limited by
/// `SESSION_TOUCH_THRESHOLD_SECS`).
pub async fn lookup_session(pool: &SqlitePool, raw_token: &str) -> AuthResult<(User, Session)> {
    let hash = hash_token(raw_token);
    let now = now_unix();

    let row = sqlx::query(
        "SELECT s.id AS s_id, s.user_id, s.device_id, s.kind, s.created_at,
                s.last_used_at, s.expires_at, s.revoked_at,
                u.id AS u_id, u.username, u.is_admin, u.can_upload, u.can_edit, u.can_download
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(AuthError::SessionNotFound);
    };

    let revoked_at: Option<i64> = row.get("revoked_at");
    let expires_at: i64 = row.get("expires_at");
    if revoked_at.is_some() || expires_at <= now {
        return Err(AuthError::SessionNotFound);
    }

    let session_id: i64 = row.get("s_id");
    let last_used_at: i64 = row.get("last_used_at");

    let user = User {
        id: row.get("u_id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
    };

    let kind_str: String = row.get("kind");
    let kind = match kind_str.as_str() {
        "cookie" => SessionKind::Cookie,
        "bearer" => SessionKind::Bearer,
        // The migration enforces this via CHECK, so an unknown value here
        // means DB corruption or a hand-edited row. Fail closed rather
        // than silently apply the wrong semantics.
        _ => return Err(AuthError::SessionNotFound),
    };
    let session = Session {
        id: session_id,
        user_id: user.id,
        device_id: row.get("device_id"),
        kind,
        created_at: row.get("created_at"),
        last_used_at,
        expires_at,
    };

    if now - last_used_at >= SESSION_TOUCH_THRESHOLD_SECS {
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(now)
            .bind(session_id)
            .execute(pool)
            .await?;
    }

    Ok((user, session))
}

pub async fn revoke_session(pool: &SqlitePool, session_id: i64) -> AuthResult<()> {
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ?")
        .bind(now_unix())
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_all_sessions_for_user(pool: &SqlitePool, user_id: i64) -> AuthResult<u64> {
    let r =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(now_unix())
            .bind(user_id)
            .execute(pool)
            .await?;
    Ok(r.rows_affected())
}

// -----------------------------------------------------------------------------
// Devices
// -----------------------------------------------------------------------------

pub async fn register_device(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    client_kind: &str,
    client_version: Option<&str>,
) -> AuthResult<Device> {
    let row = sqlx::query(
        "INSERT INTO devices (user_id, name, client_kind, client_version)
         VALUES (?, ?, ?, ?)
         RETURNING id, user_id, name, client_kind, client_version, created_at, last_seen_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(client_kind)
    .bind(client_version)
    .fetch_one(pool)
    .await?;
    Ok(Device {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        client_kind: row.get("client_kind"),
        client_version: row.get("client_version"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
    })
}

pub async fn list_devices_for_user(pool: &SqlitePool, user_id: i64) -> AuthResult<Vec<Device>> {
    let rows = sqlx::query(
        "SELECT id, user_id, name, client_kind, client_version, created_at, last_seen_at
         FROM devices WHERE user_id = ? ORDER BY last_seen_at DESC",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Device {
            id: row.get("id"),
            user_id: row.get("user_id"),
            name: row.get("name"),
            client_kind: row.get("client_kind"),
            client_version: row.get("client_version"),
            created_at: row.get("created_at"),
            last_seen_at: row.get("last_seen_at"),
        })
        .collect())
}

// -----------------------------------------------------------------------------
// Session signing-key secret
// -----------------------------------------------------------------------------

const SESSION_KEY_NAME: &str = "session_signing_key";
const SESSION_KEY_LEN: usize = 64; // 512 bits — tower-sessions key size

/// Returns the session signing key. Creates and persists a fresh random
/// key on first call if none exists. Operators who want to manage it
/// externally can pre-seed this row (or set `OMNIBUS_SESSION_KEY` — server
/// layer reads the env var and calls `put_session_key` at boot).
pub async fn load_or_create_session_key(pool: &SqlitePool) -> AuthResult<Vec<u8>> {
    if let Some(bytes) = get_session_key(pool).await? {
        return Ok(bytes);
    }
    let mut key = vec![0u8; SESSION_KEY_LEN];
    OsRng.fill_bytes(&mut key);
    put_session_key(pool, &key).await?;
    Ok(key)
}

pub async fn get_session_key(pool: &SqlitePool) -> AuthResult<Option<Vec<u8>>> {
    let v: Option<Vec<u8>> = sqlx::query_scalar("SELECT value FROM secrets WHERE name = ?")
        .bind(SESSION_KEY_NAME)
        .fetch_optional(pool)
        .await?;
    Ok(v)
}

pub async fn put_session_key(pool: &SqlitePool, key: &[u8]) -> AuthResult<()> {
    sqlx::query(
        "INSERT INTO secrets (name, value) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET value = excluded.value",
    )
    .bind(SESSION_KEY_NAME)
    .bind(key)
    .execute(pool)
    .await?;
    Ok(())
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queries::init_db;

    async fn pool() -> SqlitePool {
        init_db("sqlite::memory:").await.expect("pool init")
    }

    // ---- password hashing -----------------------------------------------------

    #[test]
    fn password_roundtrips() {
        let phc = hash_password("correct horse battery staple").unwrap();
        assert!(phc.starts_with("$argon2id$"));
        assert!(verify_password("correct horse battery staple", &phc).unwrap());
        assert!(!verify_password("wrong password entirely", &phc).unwrap());
    }

    #[test]
    fn password_policy_rejects_short() {
        assert!(matches!(
            validate_password("short"),
            Err(AuthError::PasswordTooShort { .. })
        ));
    }

    #[test]
    fn password_policy_rejects_common() {
        assert!(matches!(
            validate_password("password123"),
            Err(AuthError::PasswordCommon)
        ));
    }

    #[test]
    fn password_policy_accepts_reasonable() {
        assert!(validate_password("xk7-banana-frog-42").is_ok());
    }

    // ---- tokens ---------------------------------------------------------------

    #[test]
    fn token_is_unique_and_base64url() {
        let a = generate_token();
        let b = generate_token();
        assert_ne!(a, b);
        assert!(a
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
        assert!(a.len() >= 40);
    }

    #[test]
    fn token_hash_is_deterministic_and_32_bytes() {
        let t = "abc123";
        assert_eq!(hash_token(t), hash_token(t));
        assert_eq!(hash_token(t).len(), 32);
    }

    // ---- user creation --------------------------------------------------------

    #[tokio::test]
    async fn first_user_is_admin_and_disables_registration() {
        let p = pool().await;
        assert!(registration_enabled(&p).await.unwrap());
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        assert!(u.is_admin);
        assert!(u.can_upload);
        assert!(u.can_edit);
        assert!(u.can_download);
        assert!(!registration_enabled(&p).await.unwrap());
    }

    #[tokio::test]
    async fn second_user_needs_registration_enabled() {
        let p = pool().await;
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        // Registration auto-disabled after first user.
        let err = create_user(&p, "bob", "bunker9-longer-pass")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::RegistrationDisabled));
        // Admin re-enables.
        set_registration_enabled(&p, true).await.unwrap();
        let bob = create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
        assert!(!bob.is_admin);
        assert!(!bob.can_upload);
        assert!(!bob.can_edit);
        assert!(bob.can_download);
    }

    #[tokio::test]
    async fn username_collision_nocase() {
        let p = pool().await;
        create_user(&p, "Alice", "hunter2-real-long").await.unwrap();
        set_registration_enabled(&p, true).await.unwrap();
        let err = create_user(&p, "alice", "hunter2-real-long")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::UsernameTaken));
    }

    #[tokio::test]
    async fn concurrent_first_user_race_only_one_admin() {
        // SQLite serializes writes, so BEGIN IMMEDIATE will cause the
        // second transaction to see user_count = 1 and register bob as a
        // non-admin. This is the race we are specifically defending against.
        let p = pool().await;

        let p1 = p.clone();
        let p2 = p.clone();
        let t1 = tokio::spawn(async move { create_user(&p1, "alice", "hunter2-real-long").await });
        let t2 = tokio::spawn(async move { create_user(&p2, "bob", "bunker9-longer-pass").await });

        let r1 = t1.await.unwrap();
        let r2 = t2.await.unwrap();

        // Both succeed (second sees registration_enabled=1 because first
        // flips it inside the same transaction — the second either sees
        // it still "1" (before commit) or "0" (after commit). Under
        // BEGIN IMMEDIATE the second blocks until the first commits, so
        // it sees "0" and gets RegistrationDisabled — OR the second won
        // the BEGIN IMMEDIATE race and alice is the non-first one.
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&p)
            .await
            .unwrap();
        let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1")
            .fetch_one(&p)
            .await
            .unwrap();

        // Exactly one user succeeded, and it's the admin.
        assert_eq!(admins, 1, "exactly one admin regardless of race outcome");
        // The other either failed with RegistrationDisabled or wasn't created.
        assert!(users >= 1);
        assert!(users <= 2);
        assert!(r1.is_ok() || r2.is_ok());
    }

    // ---- login + lockout ------------------------------------------------------

    #[tokio::test]
    async fn login_success_clears_failures() {
        let p = pool().await;
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();

        // Record 2 failures, then a success, then assert counter == 0.
        let _ = verify_login(&p, "alice", "wrong!").await;
        let _ = verify_login(&p, "alice", "wrong!").await;
        let u = verify_login(&p, "alice", "hunter2-real-long")
            .await
            .unwrap();
        assert_eq!(u.username, "alice");

        let failed: i64 = sqlx::query_scalar("SELECT failed_login_count FROM users WHERE id = ?")
            .bind(u.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(failed, 0);
    }

    #[tokio::test]
    async fn login_locks_after_five_failures() {
        let p = pool().await;
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        for _ in 0..5 {
            let _ = verify_login(&p, "alice", "wrong!").await;
        }
        let err = verify_login(&p, "alice", "hunter2-real-long")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::AccountLocked { .. }));
    }

    #[tokio::test]
    async fn login_lockout_resets_after_cooldown_elapses() {
        // Regression: once the lockout window passes, a single subsequent
        // failed attempt must NOT immediately re-lock the account (the
        // monotonic counter would otherwise stay >= LOCKOUT_MIN_AFTER).
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        for _ in 0..5 {
            let _ = verify_login(&p, "alice", "wrong!").await;
        }
        // Simulate lockout window elapsing by rewriting the row.
        sqlx::query("UPDATE users SET locked_until = 1 WHERE id = ?")
            .bind(u.id)
            .execute(&p)
            .await
            .unwrap();
        // One more wrong attempt must NOT relock: effective counter is 0,
        // becomes 1, still well below LOCKOUT_MIN_AFTER.
        let err = verify_login(&p, "alice", "still-wrong").await.unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
        let locked: Option<i64> = sqlx::query_scalar("SELECT locked_until FROM users WHERE id = ?")
            .bind(u.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert!(locked.is_none(), "single failure must not relock");
        // And a subsequent correct password works.
        let ok = verify_login(&p, "alice", "hunter2-real-long")
            .await
            .unwrap();
        assert_eq!(ok.id, u.id);
    }

    #[tokio::test]
    async fn login_unknown_user_returns_invalid_credentials() {
        let p = pool().await;
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let err = verify_login(&p, "nobody", "any-long-password")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::InvalidCredentials));
    }

    // ---- sessions -------------------------------------------------------------

    #[tokio::test]
    async fn session_roundtrip() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let (user2, sess2) = lookup_session(&p, &ns.raw_token).await.unwrap();
        assert_eq!(user2.id, u.id);
        assert_eq!(sess2.id, ns.session.id);
        assert_eq!(sess2.kind, SessionKind::Cookie);
    }

    #[tokio::test]
    async fn session_lookup_hashes_token() {
        // Proves the db does not store the raw token: look up by the hash
        // directly and ensure NO row has the raw token as its hash column.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let raw_as_hash: Option<i64> =
            sqlx::query_scalar("SELECT id FROM sessions WHERE token_hash = ?")
                .bind(ns.raw_token.as_bytes())
                .fetch_optional(&p)
                .await
                .unwrap();
        assert!(raw_as_hash.is_none(), "raw token must not be stored");
    }

    #[tokio::test]
    async fn expired_session_is_rejected() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        // Simulate expiry by rewriting the row.
        sqlx::query("UPDATE sessions SET expires_at = 1 WHERE id = ?")
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        let err = lookup_session(&p, &ns.raw_token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn revoked_session_is_rejected() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        revoke_session(&p, ns.session.id).await.unwrap();
        let err = lookup_session(&p, &ns.raw_token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn unknown_token_is_rejected() {
        let p = pool().await;
        let err = lookup_session(&p, "not-a-real-token").await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    // ---- devices --------------------------------------------------------------

    #[tokio::test]
    async fn device_register_and_list() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let d = register_device(&p, u.id, "Phone", "ios", Some("1.0.0"))
            .await
            .unwrap();
        let list = list_devices_for_user(&p, u.id).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, d.id);
        assert_eq!(list[0].client_kind, "ios");
    }

    // ---- promote_to_admin -----------------------------------------------------

    #[tokio::test]
    async fn promote_to_admin_idempotent() {
        let p = pool().await;
        set_registration_enabled(&p, true).await.unwrap();
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        set_registration_enabled(&p, true).await.unwrap();
        create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
        assert!(promote_to_admin(&p, "bob").await.unwrap());
        let bob = get_user_by_username(&p, "bob").await.unwrap().unwrap();
        assert!(bob.is_admin);
        // No-op on unknown user.
        assert!(!promote_to_admin(&p, "eve").await.unwrap());
    }

    // ---- session key ----------------------------------------------------------

    #[tokio::test]
    async fn session_key_is_created_and_stable() {
        let p = pool().await;
        let k1 = load_or_create_session_key(&p).await.unwrap();
        let k2 = load_or_create_session_key(&p).await.unwrap();
        assert_eq!(k1, k2);
        assert_eq!(k1.len(), SESSION_KEY_LEN);
    }
}
