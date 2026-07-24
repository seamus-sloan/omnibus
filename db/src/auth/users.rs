//! User CRUD.

use sqlx::{SqliteConnection, SqlitePool};

use super::password::{hash_password, validate_password, validate_username};
use super::{row_to_user, AuthError, AuthResult, User};

/// Atomically create a user. The first user created becomes admin; the
/// `registration_enabled` setting is flipped to '0' in the same
/// transaction. Subsequent creates check `registration_enabled` and refuse
/// if disabled. Uses BEGIN IMMEDIATE so two concurrent callers cannot both
/// observe an empty users table.
pub async fn create_user(pool: &SqlitePool, username: &str, password: &str) -> AuthResult<User> {
    validate_username(username)?;
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

    // Runs on the transaction's connection so the RESERVED write lock from
    // `BEGIN IMMEDIATE` covers the count/uniqueness checks.
    let is_first = check_registration_preconditions(&mut tx, username).await?;

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

    // Give the new user their built-in Wishlist shelf in the same transaction,
    // so a registration either lands complete or not at all (#1187). A shelf
    // error folds to the opaque `AuthError::Internal` — the caller only
    // branches on auth-specific failures.
    crate::shelves::provision_wishlist_shelf(&mut *tx, id)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    tx.commit().await?;

    Ok(User {
        id,
        username: username.to_string(),
        is_admin: is_admin != 0,
        can_upload: can_upload != 0,
        can_edit: can_edit != 0,
        can_download: can_download != 0,
        kindle_email: None,
    })
}

/// Precondition checks for [`create_user`], run inside its `BEGIN IMMEDIATE`
/// transaction. Returns whether this is the first user (who becomes admin).
///
/// Errors with [`AuthError::RegistrationDisabled`] when the table is
/// non-empty and self-registration is off, or [`AuthError::UsernameTaken`]
/// when the (case-insensitive) username already exists. Takes the
/// transaction connection so the caller's write lock guards these reads.
async fn check_registration_preconditions(
    conn: &mut SqliteConnection,
    username: &str,
) -> AuthResult<bool> {
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

    Ok(is_first)
}

/// Look up a user record by username (case-insensitive); returns `None` if no match.
pub async fn get_user_by_username(pool: &SqlitePool, username: &str) -> AuthResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, is_admin, can_upload, can_edit, can_download, kindle_email
         FROM users WHERE username = ? COLLATE NOCASE",
    )
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

/// Look up a user record by primary key; returns `None` if no match.
pub async fn get_user_by_id(pool: &SqlitePool, id: i64) -> AuthResult<Option<User>> {
    let row = sqlx::query(
        "SELECT id, username, is_admin, can_upload, can_edit, can_download, kindle_email
         FROM users WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

/// Set (or clear, when `None`/blank) a user's Send-to-Kindle destination
/// address. Rejects a malformed address with [`AuthError::Validation`] before
/// the write so the book-detail action never targets a garbage recipient.
pub async fn set_kindle_email(
    pool: &SqlitePool,
    user_id: i64,
    email: Option<&str>,
) -> AuthResult<()> {
    let normalized = match email.map(str::trim).filter(|s| !s.is_empty()) {
        Some(e) if !omnibus_shared::is_plausible_email(e) => {
            return Err(AuthError::Validation(
                "not a valid email address".to_string(),
            ));
        }
        Some(e) => Some(e.to_string()),
        None => None,
    };
    sqlx::query("UPDATE users SET kindle_email = ? WHERE id = ?")
        .bind(normalized)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Read a user's saved Send-to-Kindle address, or `None` when unset/blank.
pub async fn get_kindle_email(pool: &SqlitePool, user_id: i64) -> AuthResult<Option<String>> {
    let v: Option<String> = sqlx::query_scalar("SELECT kindle_email FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .flatten();
    Ok(v.filter(|s| !s.trim().is_empty()))
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

/// Enable or disable new user self-registration; persists the setting to the `settings` table.
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

/// Return whether new user self-registration is currently enabled.
pub async fn registration_enabled(pool: &SqlitePool) -> AuthResult<bool> {
    let v: Option<String> =
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'registration_enabled'")
            .fetch_optional(pool)
            .await?;
    Ok(v.as_deref() == Some("1"))
}

#[cfg(test)]
mod tests;
