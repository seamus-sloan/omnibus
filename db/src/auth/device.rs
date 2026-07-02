//! Devices.

use sqlx::{Executor, Row, Sqlite, SqlitePool};

use super::{AuthError, AuthResult, Device};

/// Maximum accepted length of a `LoginRequest.device_name` /
/// `RegisterRequest.device_name`. The column is `TEXT NOT NULL`, so the
/// only natural upper bound is the global 1 MiB body limit — small per
/// request but unbounded across many calls, so an unauthenticated caller
/// could spray oversized device names at `/api/auth/register` /
/// `/login`. 255 chars matches the historical convention for "human
/// label" columns and keeps the row small for `list_devices_for_user`
/// pagination.
pub const MAX_DEVICE_NAME_CHARS: usize = 255;

/// Maximum accepted length of a `client_version`. App version strings
/// don't legitimately exceed semver + a few build-meta segments; 64
/// chars covers `1.2.3-rc.10+build.20251205abcdef` with headroom.
pub const MAX_CLIENT_VERSION_CHARS: usize = 64;

/// Reject ASCII control characters (incl. CR/LF) and oversize strings
/// before INSERT. We *reject* rather than truncate so a buggy or
/// malicious client gets a deterministic error instead of silently
/// shipping mangled data into the `devices` table — log-injection (CR
/// + crafted `tracing` line) is the specific risk this guards against.
///
/// Returns `Ok(())` for `None`: both fields are `#[serde(default)]
/// Option<String>` on the wire and absence is the legitimate path
/// (anonymous web login).
fn validate_device_field(
    value: Option<&str>,
    max_chars: usize,
    label: &'static str,
) -> AuthResult<()> {
    let Some(v) = value else {
        return Ok(());
    };
    if v.chars().count() > max_chars {
        return Err(AuthError::Validation(format!("invalid {label}: too long")));
    }
    if v.chars().any(char::is_control) {
        return Err(AuthError::Validation(format!(
            "invalid {label}: contains control characters"
        )));
    }
    Ok(())
}

/// Validate a device name against format rules (max length, no control characters); `None` is accepted.
pub fn validate_device_name(name: Option<&str>) -> AuthResult<()> {
    validate_device_field(name, MAX_DEVICE_NAME_CHARS, "device_name")
}

/// Validate a client version string against format rules (max length, no control characters); `None` is accepted.
pub fn validate_client_version(version: Option<&str>) -> AuthResult<()> {
    validate_device_field(version, MAX_CLIENT_VERSION_CHARS, "client_version")
}

/// Register a new device for a user after validating the name and client version; returns the inserted device record.
///
/// Accepts any `sqlx::Executor` so callers can pass either a `&SqlitePool` for
/// a standalone insert or `&mut *tx` from within a transaction — the login
/// path in `server::auth::handlers::issue_session` uses the latter to keep the
/// device + session inserts atomic (see #627: an orphan device row leaks if
/// `create_session` fails after `register_device` has already committed).
pub async fn register_device<'e, E>(
    executor: E,
    user_id: i64,
    name: &str,
    client_kind: &str,
    client_version: Option<&str>,
) -> AuthResult<Device>
where
    E: Executor<'e, Database = Sqlite>,
{
    // Guards run *before* the INSERT so a rejected field never reaches
    // SQLite. Mirrors the placement of `validate_password` in `users`.
    validate_device_name(Some(name))?;
    validate_client_version(client_version)?;
    let row = sqlx::query(
        "INSERT INTO devices (user_id, name, client_kind, client_version)
         VALUES (?, ?, ?, ?)
         RETURNING id, user_id, name, client_kind, client_version, created_at, last_seen_at",
    )
    .bind(user_id)
    .bind(name)
    .bind(client_kind)
    .bind(client_version)
    .fetch_one(executor)
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

/// List all registered devices for a user, ordered by most-recently-seen first.
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

#[cfg(test)]
mod tests;
