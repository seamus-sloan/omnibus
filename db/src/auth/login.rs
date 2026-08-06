//! Login.

use sqlx::{Row, SqlitePool};

use super::password::{hash_password, verify_password};
use super::{now_unix, row_to_user, AuthError, AuthResult, User};

/// Lockout schedule (minutes), keyed on the number of prior lockouts. After
/// 5 failed attempts in any window we consult this table for the next
/// `locked_until`.
const LOCKOUT_MIN_AFTER: i64 = 5;
const LOCKOUT_DURATION_SECS: i64 = 15 * 60;

/// Sentinel PHC string used when the username is unknown, so we still
/// spend ~250ms in argon2 verify and don't leak username existence via
/// response-timing. Generated lazily on first miss; an argon2 failure
/// here is surfaced as `AuthError::Crypto` rather than crashing every
/// subsequent login.
fn sentinel_hash() -> AuthResult<&'static str> {
    use std::sync::OnceLock;
    static HASH: OnceLock<String> = OnceLock::new();
    if let Some(h) = HASH.get() {
        return Ok(h.as_str());
    }
    let hashed = hash_password("__timing_equalizer_not_a_real_password__")?;
    // The first racer wins; either the inserted value or the value already
    // present is fine because both are valid PHC strings for the same
    // sentinel password.
    Ok(HASH.get_or_init(|| hashed).as_str())
}

/// Verify a login attempt. On success returns the user; on failure returns
/// a generic `InvalidCredentials` (same error for unknown username and
/// wrong password). Enforces per-account lockout.
pub async fn verify_login(pool: &SqlitePool, username: &str, password: &str) -> AuthResult<User> {
    let row = sqlx::query(
        "SELECT u.id, u.username, u.password_hash, u.is_admin, u.can_upload, u.can_edit,
                u.can_download, u.kindle_email, u.display_name,
                EXISTS(SELECT 1 FROM user_avatars a WHERE a.user_id = u.id) AS has_avatar,
                u.failed_login_count, u.locked_until
         FROM users u WHERE u.username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;

    let now = now_unix();

    let Some(row) = row else {
        // Equalize timing against the found-user path. A sentinel-hash
        // failure here is treated as `AuthError::Crypto` (500) — better
        // than swallowing it and silently leaking timing info.
        let _ = verify_password(password, sentinel_hash()?);
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

#[cfg(test)]
mod tests;
