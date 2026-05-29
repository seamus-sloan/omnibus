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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::pool;
    use crate::auth::users::create_user;

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
}
