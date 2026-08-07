//! User CRUD.

use omnibus_shared::{AdminUserRow, UserPermissions};
use sqlx::{Row, SqliteConnection, SqlitePool};

use super::password::{hash_password, validate_password, validate_username, verify_password};
use super::session::{revoke_all_sessions_for_user, revoke_all_sessions_for_user_except};
use super::{now_unix, row_to_user, AuthError, AuthResult, User};

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
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
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

/// The projection [`row_to_user`] expects, over a `users` table aliased `u`.
/// `has_avatar` is derived rather than stored so it cannot drift from the
/// `user_avatars` rows it describes.
pub(crate) const USER_COLUMNS: &str =
    "u.id, u.username, u.is_admin, u.can_upload, u.can_edit, u.can_download,
     u.kindle_email, u.display_name, u.hidden_formats,
     EXISTS(SELECT 1 FROM user_avatars a WHERE a.user_id = u.id) AS has_avatar";

/// Look up a user record by username (case-insensitive); returns `None` if no match.
pub async fn get_user_by_username(pool: &SqlitePool, username: &str) -> AuthResult<Option<User>> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM users u WHERE u.username = ? COLLATE NOCASE"
    ))
    .bind(username)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_user))
}

/// Look up a user record by primary key; returns `None` if no match.
pub async fn get_user_by_id(pool: &SqlitePool, id: i64) -> AuthResult<Option<User>> {
    let row = sqlx::query(&format!(
        "SELECT {USER_COLUMNS} FROM users u WHERE u.id = ?"
    ))
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

/// Longest accepted display name. Generous enough for a real name, short
/// enough that it can't blow out a shelf title or a journal byline.
pub const DISPLAY_NAME_MAX_LEN: usize = 64;

/// Set (or clear, when `None`/blank) a user's display name, and rename their
/// Wishlist shelf to match in the same transaction.
///
/// The rename is not incidental: `shelves.name` denormalizes
/// `COALESCE(display_name, username) || "'s Wishlist"` at provisioning time, so
/// leaving it alone would strand the old label forever. Doing both writes under
/// one transaction keeps the stored name and the name it was derived from from
/// ever disagreeing. No unique-index risk — `idx_shelves_owner_name` excludes
/// `kind = 'wishlist'`.
///
/// Errors with [`AuthError::Validation`] for a name that is too long or carries
/// control characters (which would break the shelf title and byline rendering).
pub async fn set_display_name(
    pool: &SqlitePool,
    user_id: i64,
    name: Option<&str>,
) -> AuthResult<()> {
    let normalized = match name.map(str::trim).filter(|s| !s.is_empty()) {
        Some(n) if n.chars().count() > DISPLAY_NAME_MAX_LEN => {
            return Err(AuthError::Validation(format!(
                "display name must be {DISPLAY_NAME_MAX_LEN} characters or fewer"
            )));
        }
        Some(n) if n.chars().any(char::is_control) => {
            return Err(AuthError::Validation(
                "display name cannot contain control characters".to_string(),
            ));
        }
        Some(n) => Some(n.to_string()),
        None => None,
    };

    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE users SET display_name = ? WHERE id = ?")
        .bind(&normalized)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE shelves
            SET name = (SELECT COALESCE(u.display_name, u.username) FROM users u WHERE u.id = ?1) || ?2
          WHERE owner_user_id = ?1 AND kind = 'wishlist'",
    )
    .bind(user_id)
    .bind(crate::shelves::provision::WISHLIST_NAME_SUFFIX)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
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

/// Most format tokens a user may hide — far above any real library's format
/// variety, low enough that the column can't become a dumping ground.
pub const HIDDEN_FORMATS_MAX: usize = 32;

/// Set (or clear, when empty) the formats a user hides from the landing
/// All Books view. Tokens are canonicalized (trim, lowercase, dedupe, sort)
/// before storage and the canonical list is returned. Rejects a token that
/// isn't a plausible file-format name (`[a-z0-9]{1,16}` after lowercasing)
/// or a list longer than [`HIDDEN_FORMATS_MAX`] with
/// [`AuthError::Validation`].
pub async fn set_hidden_formats(
    pool: &SqlitePool,
    user_id: i64,
    formats: &[String],
) -> AuthResult<Vec<String>> {
    let mut canonical: Vec<String> = formats
        .iter()
        .map(|f| f.trim().to_lowercase())
        .filter(|f| !f.is_empty())
        .collect();
    canonical.sort();
    canonical.dedup();

    if canonical.len() > HIDDEN_FORMATS_MAX {
        return Err(AuthError::Validation(format!(
            "at most {HIDDEN_FORMATS_MAX} hidden formats are supported"
        )));
    }
    if let Some(bad) = canonical.iter().find(|f| {
        f.len() > 16
            || !f
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
    }) {
        return Err(AuthError::Validation(format!(
            "not a valid format name: {bad:?}"
        )));
    }

    let stored = if canonical.is_empty() {
        None
    } else {
        Some(canonical.join(","))
    };
    sqlx::query("UPDATE users SET hidden_formats = ? WHERE id = ?")
        .bind(stored)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(canonical)
}

/// Read a user's hidden-formats list, canonical order. Empty when unset.
pub async fn get_hidden_formats(pool: &SqlitePool, user_id: i64) -> AuthResult<Vec<String>> {
    let v: Option<Option<String>> =
        sqlx::query_scalar("SELECT hidden_formats FROM users WHERE id = ?")
            .bind(user_id)
            .fetch_optional(pool)
            .await?;
    Ok(crate::auth::parse_hidden_formats(v.flatten()))
}

/// Change a user's password. Verifies `current` against the stored hash to
/// authorize the change, validates `new` against the password policy, then
/// re-hashes with Argon2id and stamps `password_changed_at` — all in one
/// transaction so an interrupted call never leaves a half-applied state.
/// Also revokes every other active session for the user in the same
/// transaction (#1402), except `except_session_id` — the caller's own
/// session, which stays live so a self-service change doesn't immediately
/// log the caller out.
///
/// Errors with [`AuthError::InvalidCredentials`] when the current password is
/// wrong (or the user id no longer exists) and [`AuthError::Validation`] when
/// the new password fails policy (min length / common-password reject-list).
/// Only ever touches the row identified by `user_id`.
pub async fn change_password(
    pool: &SqlitePool,
    user_id: i64,
    current: &str,
    new: &str,
    except_session_id: i64,
) -> AuthResult<()> {
    let mut tx = pool.begin().await?;

    let phc: Option<String> = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *tx)
        .await?;
    let Some(phc) = phc else {
        return Err(AuthError::InvalidCredentials);
    };
    if !verify_password(current, &phc)? {
        return Err(AuthError::InvalidCredentials);
    }

    // Validate only after authorizing, so an unauthenticated caller can't
    // probe the policy — and reject before hashing to avoid wasted argon2 work.
    validate_password(new)?;
    let new_phc = hash_password(new)?;

    // Stamp `password_changed_at` alongside the new hash (the standing
    // INVARIANT in `create_user`): downstream "invalidate sessions older than
    // last password change" logic reads this column.
    let updated = sqlx::query(
        "UPDATE users SET password_hash = ?, password_changed_at = strftime('%s','now')
         WHERE id = ?",
    )
    .bind(&new_phc)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    // The transaction pins the row we verified above, so a 0-row update
    // shouldn't happen — but never commit a no-op as success. Treat it as
    // `InvalidCredentials`, consistent with the missing-user path.
    if updated.rows_affected() == 0 {
        return Err(AuthError::InvalidCredentials);
    }

    // Kick out anyone else holding a session for this account (e.g. an
    // attacker with a stolen cookie) — but leave the caller's own session
    // live, since they authenticated this very request with it.
    revoke_all_sessions_for_user_except(&mut *tx, user_id, except_session_id).await?;

    tx.commit().await?;
    Ok(())
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

// ── Admin user management (F5.4) ─────────────────────────────────────

/// Map a `users` row to the admin-table [`AdminUserRow`] projection,
/// resolving `locked` against `now` (a `locked_until` in the past is not a
/// live lockout).
fn row_to_admin_user(row: &sqlx::sqlite::SqliteRow, now: i64) -> AdminUserRow {
    let locked_until: Option<i64> = row.get("locked_until");
    AdminUserRow {
        id: row.get("id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
        kindle_email: row.get("kindle_email"),
        display_name: row.get("display_name"),
        created_at: row.get("created_at"),
        locked: locked_until.is_some_and(|until| until > now),
    }
}

/// List every user for the admin Users table, oldest first. Returns the
/// safe [`AdminUserRow`] projection (no password material) with per-row
/// created time and live locked state.
pub async fn list_users(pool: &SqlitePool) -> AuthResult<Vec<AdminUserRow>> {
    let rows = sqlx::query(
        "SELECT id, username, is_admin, can_upload, can_edit, can_download,
                kindle_email, display_name, created_at, locked_until
         FROM users ORDER BY created_at ASC, id ASC",
    )
    .fetch_all(pool)
    .await?;
    let now = now_unix();
    Ok(rows.iter().map(|r| row_to_admin_user(r, now)).collect())
}

/// Admin-create a user with explicit permissions. Unlike [`create_user`]
/// (self-registration), this bypasses the `registration_enabled` gate and the
/// first-user-admin logic — an admin is always the caller — and sets the four
/// permission flags directly. Validates username/password, provisions the
/// built-in Wishlist shelf (like `create_user`, #1187), and returns the new
/// row. Errors with [`AuthError::UsernameTaken`] on a case-insensitive
/// collision.
pub async fn admin_create_user(
    pool: &SqlitePool,
    username: &str,
    password: &str,
    perms: UserPermissions,
) -> AuthResult<AdminUserRow> {
    validate_username(username)?;
    validate_password(password)?;
    let phc = hash_password(password)?;

    // BEGIN IMMEDIATE takes the write lock up front so the uniqueness check and
    // the insert can't race a concurrent create of the same name.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let existing: Option<i64> =
        sqlx::query_scalar("SELECT id FROM users WHERE username = ? COLLATE NOCASE")
            .bind(username)
            .fetch_optional(&mut *tx)
            .await?;
    if existing.is_some() {
        return Err(AuthError::UsernameTaken);
    }

    let id: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(username)
    .bind(&phc)
    .bind(perms.is_admin as i64)
    .bind(perms.can_upload as i64)
    .bind(perms.can_edit as i64)
    .bind(perms.can_download as i64)
    .fetch_one(&mut *tx)
    .await?;

    crate::shelves::provision_wishlist_shelf(&mut *tx, id)
        .await
        .map_err(|e| AuthError::Internal(e.to_string()))?;

    tx.commit().await?;

    Ok(AdminUserRow {
        id,
        username: username.to_string(),
        is_admin: perms.is_admin,
        can_upload: perms.can_upload,
        can_edit: perms.can_edit,
        can_download: perms.can_download,
        kindle_email: None,
        display_name: None,
        created_at: now_unix(),
        locked: false,
    })
}

/// Replace a user's four permission flags. Refuses to demote the last
/// remaining admin ([`AuthError::LastAdmin`]) and errors with
/// [`AuthError::UserNotFound`] for an unknown id. Runs under `BEGIN
/// IMMEDIATE` so the admin-count guard can't race a concurrent demotion of a
/// different admin down to zero admins.
pub async fn update_user_permissions(
    pool: &SqlitePool,
    user_id: i64,
    perms: UserPermissions,
) -> AuthResult<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let current_is_admin = admin_flag_for_update(&mut tx, user_id).await?;

    // Demoting the only admin would lock everyone out of the admin panel.
    if current_is_admin && !perms.is_admin && admin_count(&mut tx).await? <= 1 {
        return Err(AuthError::LastAdmin);
    }

    sqlx::query(
        "UPDATE users SET is_admin = ?, can_upload = ?, can_edit = ?, can_download = ?
         WHERE id = ?",
    )
    .bind(perms.is_admin as i64)
    .bind(perms.can_upload as i64)
    .bind(perms.can_edit as i64)
    .bind(perms.can_download as i64)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(())
}

/// Admin-reset a user's password: validate + re-hash + stamp
/// `password_changed_at`, then revoke every active session for the target
/// user in the same transaction (#1402) — unlike the self-service
/// [`change_password`], there is no caller session to exclude, since the
/// admin isn't the affected account. No current-password check — this is
/// the admin override path.
/// [`AuthError::UserNotFound`] for an unknown id; [`AuthError::Validation`]
/// when the new password fails policy.
pub async fn admin_set_password(
    pool: &SqlitePool,
    user_id: i64,
    new_password: &str,
) -> AuthResult<()> {
    validate_password(new_password)?;
    let phc = hash_password(new_password)?;

    let mut tx = pool.begin().await?;

    let updated = sqlx::query(
        "UPDATE users SET password_hash = ?, password_changed_at = strftime('%s','now')
         WHERE id = ?",
    )
    .bind(&phc)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    if updated.rows_affected() == 0 {
        return Err(AuthError::UserNotFound);
    }

    revoke_all_sessions_for_user(&mut *tx, user_id).await?;

    tx.commit().await?;
    Ok(())
}

/// Clear a user's lockout: reset `failed_login_count` and `locked_until`.
/// A no-op unlock of an already-unlocked user still succeeds;
/// [`AuthError::UserNotFound`] only for an unknown id.
pub async fn unlock_user(pool: &SqlitePool, user_id: i64) -> AuthResult<()> {
    let updated =
        sqlx::query("UPDATE users SET failed_login_count = 0, locked_until = NULL WHERE id = ?")
            .bind(user_id)
            .execute(pool)
            .await?;
    if updated.rows_affected() == 0 {
        return Err(AuthError::UserNotFound);
    }
    Ok(())
}

/// Delete a user. Their owned rows cascade away via the schema's
/// `ON DELETE CASCADE` foreign keys (sessions, devices, progress, shelves,
/// ratings, journals, …); audit references (`updated_by`/`merged_by`/
/// `added_by`) null out. Refuses to delete the last remaining admin
/// ([`AuthError::LastAdmin`]); [`AuthError::UserNotFound`] for an unknown id.
/// Runs under `BEGIN IMMEDIATE` so the admin-count guard is race-free.
pub async fn delete_user(pool: &SqlitePool, user_id: i64) -> AuthResult<()> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let is_admin = admin_flag_for_update(&mut tx, user_id).await?;
    if is_admin && admin_count(&mut tx).await? <= 1 {
        return Err(AuthError::LastAdmin);
    }

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;
    Ok(())
}

/// Read a user's current `is_admin` flag inside a write transaction, erroring
/// with [`AuthError::UserNotFound`] when the id is unknown. Shared by the
/// permission-update and delete guards.
async fn admin_flag_for_update(conn: &mut SqliteConnection, user_id: i64) -> AuthResult<bool> {
    let flag: Option<i64> = sqlx::query_scalar("SELECT is_admin FROM users WHERE id = ?")
        .bind(user_id)
        .fetch_optional(&mut *conn)
        .await?;
    flag.map(|v| v != 0).ok_or(AuthError::UserNotFound)
}

/// Count admin users on the transaction's connection (for the last-admin guard).
async fn admin_count(conn: &mut SqliteConnection) -> AuthResult<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1")
            .fetch_one(&mut *conn)
            .await?,
    )
}

#[cfg(test)]
mod tests;
