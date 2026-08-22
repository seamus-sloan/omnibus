//! Per-device Kobo wireless-sync auth tokens. Each row is one registered Kobo
//! owned by a `user_id`, carrying a re-displayable path token the reader pastes
//! into its `api_endpoint`. The token is stored in the clear because it must be
//! shown again (AC2); the delta `sync_cursor` lives here too, independent of the
//! token, so a regenerate never disturbs sync state. See migration 0053.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::auth::{generate_token, AuthResult};

/// Column list shared by every row-returning query, matched by [`row_to_device`].
const COLS: &str =
    "id, user_id, token, name, kobo_device_id, sync_cursor, created_at, last_seen_at";

/// Hard cap on devices `list_devices` returns for one user (matches `LIST_SHELVES_LIMIT`/`LIST_BOOKMARKS_LIMIT`).
pub const LIST_DEVICES_LIMIT: i64 = 100;

/// One registered Kobo: its owning user, the re-displayable path token, a
/// user-visible label, the learned `x-kobo-deviceid` (once seen), the opaque
/// per-device delta cursor, and timestamps.
#[derive(Debug, Clone, Serialize)]
pub struct KoboDevice {
    pub id: i64,
    pub user_id: i64,
    pub token: String,
    pub name: String,
    pub kobo_device_id: Option<String>,
    pub sync_cursor: Option<String>,
    pub created_at: i64,
    pub last_seen_at: i64,
}

/// Principal resolved from a `/kobo/<TOKEN>/…` path token: which device row and
/// which user it belongs to. The Kobo path-token extractor builds its
/// authenticated user from this.
#[derive(Debug, Clone)]
pub struct ResolvedKoboDevice {
    pub device_id: i64,
    pub user_id: i64,
}

/// Failure space for Kobo device registration and lookup.
#[derive(Debug, thiserror::Error)]
pub enum KoboDeviceError {
    #[error("kobo device not found")]
    NotFound,
    #[error("token generation failed: {0}")]
    Token(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// Register a new Kobo for `user_id`, minting a fresh token. Returns the full
/// row (including the raw token — the caller shows it in the Settings card).
pub async fn create_device(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
) -> Result<KoboDevice, KoboDeviceError> {
    create_device_with_token(pool, user_id, name, generate_token).await
}

/// `create_device` with the token generator passed in, so a test can inject a
/// forced failure without depending on the OS CSPRNG actually erroring.
async fn create_device_with_token(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    token_fn: impl FnOnce() -> AuthResult<String>,
) -> Result<KoboDevice, KoboDeviceError> {
    let token = token_fn().map_err(|e| KoboDeviceError::Token(e.to_string()))?;
    let row = sqlx::query(&format!(
        "INSERT INTO kobo_devices (user_id, token, name)
         VALUES (?, ?, ?)
         RETURNING {COLS}"
    ))
    .bind(user_id)
    .bind(&token)
    .bind(name)
    .fetch_one(pool)
    .await?;
    Ok(row_to_device(&row))
}

/// List every Kobo registered by `user_id`, oldest first. Scoped to the user
/// (AC4) — one user never sees another's devices.
pub async fn list_devices(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<KoboDevice>, KoboDeviceError> {
    let rows = sqlx::query(&format!(
        "SELECT {COLS} FROM kobo_devices
         WHERE user_id = ?
         ORDER BY created_at ASC, id ASC
         LIMIT ?"
    ))
    .bind(user_id)
    .bind(LIST_DEVICES_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_device).collect())
}

/// Fetch a single device by id, scoped to its owner. `Ok(None)` when the id is
/// unknown or belongs to another user (AC4).
pub async fn get_device_for_user(
    pool: &SqlitePool,
    user_id: i64,
    device_id: i64,
) -> Result<Option<KoboDevice>, KoboDeviceError> {
    let row = sqlx::query(&format!(
        "SELECT {COLS} FROM kobo_devices WHERE id = ? AND user_id = ?"
    ))
    .bind(device_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().map(row_to_device))
}

/// Rotate a device's token in place, invalidating the prior one (AC2). The
/// `name`, learned `kobo_device_id`, and `sync_cursor` are left untouched, so a
/// regenerate never disturbs delta state (AC3). `NotFound` when the id is
/// unknown or not owned by `user_id`.
pub async fn regenerate_token(
    pool: &SqlitePool,
    user_id: i64,
    device_id: i64,
) -> Result<KoboDevice, KoboDeviceError> {
    let token = generate_token().map_err(|e| KoboDeviceError::Token(e.to_string()))?;
    let row = sqlx::query(&format!(
        "UPDATE kobo_devices SET token = ?
         WHERE id = ? AND user_id = ?
         RETURNING {COLS}"
    ))
    .bind(&token)
    .bind(device_id)
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    row.as_ref()
        .map(row_to_device)
        .ok_or(KoboDeviceError::NotFound)
}

/// Remove a registered Kobo, revoking its token. `NotFound` when the id is
/// unknown or not owned by `user_id` (AC4).
pub async fn revoke_device(
    pool: &SqlitePool,
    user_id: i64,
    device_id: i64,
) -> Result<(), KoboDeviceError> {
    let res = sqlx::query("DELETE FROM kobo_devices WHERE id = ? AND user_id = ?")
        .bind(device_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(KoboDeviceError::NotFound);
    }
    Ok(())
}

/// Resolve a raw path token to its owning device + user, touching
/// `last_seen_at`. `Ok(None)` for an unknown (invalid or revoked) token — the
/// extractor maps that to `401`. This is the authentication hot path for every
/// wireless-sync request.
pub async fn resolve_device_by_token(
    pool: &SqlitePool,
    token: &str,
) -> Result<Option<ResolvedKoboDevice>, KoboDeviceError> {
    // Validate the token, touch `last_seen_at`, and return the ids in one
    // roundtrip — this runs on every wireless-sync request.
    let Some(row) = sqlx::query(
        "UPDATE kobo_devices SET last_seen_at = strftime('%s', 'now')
         WHERE token = ?
         RETURNING id, user_id",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ResolvedKoboDevice {
        device_id: row.get("id"),
        user_id: row.get("user_id"),
    }))
}

/// Bind the `x-kobo-deviceid` hardware id to this device row, so token-less
/// Reading Services calls (#1278) can map back to it. Steal-on-learn: the id
/// is cleared from any other row first (0060 enforces uniqueness), so the
/// hardware id always follows the device row whose *token* it most recently
/// presented — a factory-reset or re-pointed Kobo re-binds correctly, and no
/// other user's row can retain it.
pub async fn learn_kobo_device_id(
    pool: &SqlitePool,
    device_id: i64,
    hardware_id: &str,
) -> Result<(), KoboDeviceError> {
    let mut tx = pool.begin().await?;
    // Clear before set, in that order, so the partial unique index never trips.
    sqlx::query(
        "UPDATE kobo_devices SET kobo_device_id = NULL WHERE kobo_device_id = ? AND id <> ?",
    )
    .bind(hardware_id)
    .bind(device_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query("UPDATE kobo_devices SET kobo_device_id = ? WHERE id = ?")
        .bind(hardware_id)
        .bind(device_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

/// Resolve an `x-kobo-deviceid` hardware id to its owning device + user,
/// touching `last_seen_at`. `Ok(None)` for an id no tokened call has taught us
/// — the Reading Services extractor maps that to `401`. At most one row can
/// match (unique index, 0060).
pub async fn resolve_device_by_hardware_id(
    pool: &SqlitePool,
    hardware_id: &str,
) -> Result<Option<ResolvedKoboDevice>, KoboDeviceError> {
    let Some(row) = sqlx::query(
        "UPDATE kobo_devices SET last_seen_at = strftime('%s', 'now')
         WHERE kobo_device_id = ?
         RETURNING id, user_id",
    )
    .bind(hardware_id)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ResolvedKoboDevice {
        device_id: row.get("id"),
        user_id: row.get("user_id"),
    }))
}

/// Store the opaque per-device delta cursor (#926). Kept independent of the
/// auth token so a regenerate never resets sync state (AC3).
pub async fn set_sync_cursor(
    pool: &SqlitePool,
    device_id: i64,
    cursor: &str,
) -> Result<(), KoboDeviceError> {
    sqlx::query("UPDATE kobo_devices SET sync_cursor = ? WHERE id = ?")
        .bind(cursor)
        .bind(device_id)
        .execute(pool)
        .await?;
    Ok(())
}

fn row_to_device(row: &sqlx::sqlite::SqliteRow) -> KoboDevice {
    KoboDevice {
        id: row.get("id"),
        user_id: row.get("user_id"),
        token: row.get("token"),
        name: row.get("name"),
        kobo_device_id: row.get("kobo_device_id"),
        sync_cursor: row.get("sync_cursor"),
        created_at: row.get("created_at"),
        last_seen_at: row.get("last_seen_at"),
    }
}

#[cfg(test)]
mod tests;
