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
