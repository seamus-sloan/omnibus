//! Devices.

use sqlx::{Row, SqlitePool};

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
        return Err(AuthError::DeviceFieldInvalid {
            field: label,
            reason: "too long",
        });
    }
    if v.chars().any(|c| c.is_control()) {
        return Err(AuthError::DeviceFieldInvalid {
            field: label,
            reason: "contains control characters",
        });
    }
    Ok(())
}

pub fn validate_device_name(name: Option<&str>) -> AuthResult<()> {
    validate_device_field(name, MAX_DEVICE_NAME_CHARS, "device_name")
}

pub fn validate_client_version(version: Option<&str>) -> AuthResult<()> {
    validate_device_field(version, MAX_CLIENT_VERSION_CHARS, "client_version")
}

pub async fn register_device(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
    client_kind: &str,
    client_version: Option<&str>,
) -> AuthResult<Device> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::pool;
    use crate::auth::users::create_user;

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

    #[test]
    fn validate_device_name_accepts_short_unicode_label() {
        // Char count, not byte count: a 4-byte emoji is one character.
        let s = "📱 Phone — Alice";
        assert!(validate_device_name(Some(s)).is_ok());
        assert!(validate_device_name(None).is_ok());
    }

    #[test]
    fn validate_device_name_rejects_overlength() {
        let too_long: String = "a".repeat(MAX_DEVICE_NAME_CHARS + 1);
        let err = validate_device_name(Some(&too_long)).unwrap_err();
        match err {
            AuthError::DeviceFieldInvalid { field, .. } => assert_eq!(field, "device_name"),
            other => panic!("expected DeviceFieldInvalid, got {other:?}"),
        }
    }

    #[test]
    fn validate_device_name_at_exact_cap_passes() {
        let exact: String = "a".repeat(MAX_DEVICE_NAME_CHARS);
        assert!(validate_device_name(Some(&exact)).is_ok());
    }

    #[test]
    fn validate_device_name_rejects_control_characters() {
        // CR/LF guard the log-injection vector — without it a `device_name`
        // of `"benign\nFAKE LOG LINE"` would split a tracing event into two.
        for raw in ["benign\nlog", "tab\there", "null\0byte", "bell\x07"] {
            let err = validate_device_name(Some(raw)).unwrap_err();
            assert!(
                matches!(err, AuthError::DeviceFieldInvalid { .. }),
                "expected control-char rejection for {raw:?}, got {err:?}",
            );
        }
    }

    #[test]
    fn validate_client_version_caps_at_64_chars() {
        assert!(validate_client_version(Some("1.2.3-rc.10+build.abc")).is_ok());
        let too_long: String = "1".repeat(MAX_CLIENT_VERSION_CHARS + 1);
        let err = validate_client_version(Some(&too_long)).unwrap_err();
        assert!(
            matches!(err, AuthError::DeviceFieldInvalid { field, .. } if field == "client_version")
        );
    }

    #[tokio::test]
    async fn register_device_rejects_overlong_name_before_insert() {
        // No row is inserted on rejection — the validator runs before the
        // SQL roundtrip, so list_devices_for_user stays empty.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let too_long: String = "x".repeat(MAX_DEVICE_NAME_CHARS + 1);
        let err = register_device(&p, u.id, &too_long, "ios", None)
            .await
            .unwrap_err();
        assert!(matches!(err, AuthError::DeviceFieldInvalid { .. }));
        let list = list_devices_for_user(&p, u.id).await.unwrap();
        assert!(list.is_empty(), "rejection must not leave a partial row");
    }

    #[tokio::test]
    async fn register_device_rejects_control_char_in_client_version() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let err = register_device(&p, u.id, "Phone", "ios", Some("1.0\n0"))
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            AuthError::DeviceFieldInvalid { field, .. } if field == "client_version"
        ));
    }
}
