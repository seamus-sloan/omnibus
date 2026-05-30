//! User CRUD.

use sqlx::SqlitePool;

use super::password::{hash_password, validate_password};
use super::{row_to_user, AuthError, AuthResult, User};

/// Atomically create a user. The first user created becomes admin; the
/// `registration_enabled` setting is flipped to '0' in the same
/// transaction. Subsequent creates check `registration_enabled` and refuse
/// if disabled. Uses BEGIN IMMEDIATE so two concurrent callers cannot both
/// observe an empty users table.
pub async fn create_user(pool: &SqlitePool, username: &str, password: &str) -> AuthResult<User> {
    validate_password(password)?;
    let phc = hash_password(password)?;

    // `BEGIN IMMEDIATE` is load-bearing here: it takes a RESERVED write lock
    // at transaction start, so two concurrent first-user registrations can't
    // both observe an empty `users` table and both become admin. Plain
    // `pool.begin()` issues a DEFERRED `BEGIN` that acquires the write lock
    // lazily, which would weaken that guarantee — so we use `begin_with` to
    // issue the exact statement while still getting a real `sqlx::Transaction`
    // (structured ROLLBACK on early-return drop, no reliance on
    // connection-drop implicit cleanup).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let user_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&mut *tx)
        .await?;

    let is_first = user_count == 0;

    if !is_first {
        let enabled: String =
            sqlx::query_scalar("SELECT value FROM settings WHERE key = 'registration_enabled'")
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or_else(|| "0".to_string());
        if enabled != "1" {
            return Err(AuthError::RegistrationDisabled);
        }
    }

    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE username = ? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&mut *tx)
            .await?;
    if existing.is_some() {
        return Err(AuthError::UsernameTaken);
    }

    let is_admin = if is_first { 1i64 } else { 0 };
    let can_upload = if is_first { 1i64 } else { 0 };
    let can_edit = if is_first { 1i64 } else { 0 };
    let can_download = 1i64;

    // INVARIANT: update `password_changed_at` here when password changes.
    // The column defaults to `strftime('%s','now')` on INSERT (see
    // `migrations/0004_auth.sql`), which is correct for first creation —
    // but any future password-change endpoint MUST issue
    // `UPDATE users SET password_changed_at = strftime('%s','now')
    //  WHERE id = ?` in the same transaction as the new `password_hash`,
    // otherwise downstream "invalidate sessions older than last password
    // change" logic will silently read the account-creation timestamp.
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
    .fetch_one(&mut *tx)
    .await?;

    if is_first {
        sqlx::query("UPDATE settings SET value = '0' WHERE key = 'registration_enabled'")
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    Ok(User {
        id,
        username: username.to_string(),
        is_admin: is_admin != 0,
        can_upload: can_upload != 0,
        can_edit: can_edit != 0,
        can_download: can_download != 0,
    })
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::pool;

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
        // BEGIN IMMEDIATE serializes the two registrations on the RESERVED
        // write lock: whichever transaction wins inserts the first user as
        // admin and flips registration_enabled to '0' before committing. The
        // loser blocks until that commit, then sees user_count = 1 with
        // registration disabled and is rejected. So the race resolves
        // deterministically — exactly one user, who is the admin, and exactly
        // one Ok. This is the race we are specifically defending against.
        let p = pool().await;

        let p1 = p.clone();
        let p2 = p.clone();
        let t1 = tokio::spawn(async move { create_user(&p1, "alice", "hunter2-real-long").await });
        let t2 = tokio::spawn(async move { create_user(&p2, "bob", "bunker9-longer-pass").await });

        let r1 = t1.await.unwrap();
        let r2 = t2.await.unwrap();

        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&p)
            .await
            .unwrap();
        let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1")
            .fetch_one(&p)
            .await
            .unwrap();

        // The loser is rejected before inserting, so exactly one user exists
        // and it's the admin; exactly one create returned Ok.
        assert_eq!(users, 1, "loser never inserts — exactly one user survives");
        assert_eq!(admins, 1, "exactly one admin regardless of race outcome");
        assert!(
            r1.is_ok() ^ r2.is_ok(),
            "exactly one create succeeds; the other is rejected"
        );
    }

    #[tokio::test]
    async fn create_user_error_path_rolls_back_cleanly() {
        // A `?` early-return between BEGIN IMMEDIATE and COMMIT (here a
        // UsernameTaken reject) must drop the transaction without a partial
        // commit: no extra user row, `registration_enabled` untouched, and a
        // subsequent valid create still succeeds on the same connection pool.
        let p = pool().await;
        create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        set_registration_enabled(&p, true).await.unwrap();

        // Collide on the existing username — returns inside the transaction.
        let err = create_user(&p, "alice", "different-long-pass")
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::UsernameTaken));

        // The failed attempt left no trace: still exactly one user, and the
        // admin's registration toggle wasn't flipped by the rolled-back tx.
        let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(users, 1, "rolled-back create must not insert a row");
        assert!(
            registration_enabled(&p).await.unwrap(),
            "registration toggle must survive the rollback"
        );

        // The connection is back in a usable, non-stuck state.
        let bob = create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
        assert!(!bob.is_admin);
    }

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
}
