//! Per-user avatar bytes: one row per user, replaced in place on upload.
//!
//! Stored in the DB rather than on disk because an avatar is small, has exactly
//! one owner, and is always replaced rather than accumulated — an upsert covers
//! the whole lifecycle with no orphan sweep and no extra directory to configure.

use sqlx::{Row, SqlitePool};

use super::{now_unix, AuthResult};

/// A stored avatar: its sniffed content type and the image bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAvatar {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Read a user's avatar, or `None` when they haven't set one.
pub async fn get_user_avatar(pool: &SqlitePool, user_id: i64) -> AuthResult<Option<UserAvatar>> {
    let row = sqlx::query("SELECT mime, bytes FROM user_avatars WHERE user_id = ?")
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| UserAvatar {
        mime: r.get("mime"),
        bytes: r.get("bytes"),
    }))
}

/// Store a user's avatar, replacing any existing one.
///
/// `mime` must be the *sniffed* type from `image_upload::extract_validated_image`,
/// not the client's header — it is echoed back as the `Content-Type` on every
/// later read.
pub async fn upsert_user_avatar(
    pool: &SqlitePool,
    user_id: i64,
    mime: &str,
    bytes: &[u8],
) -> AuthResult<()> {
    sqlx::query(
        "INSERT INTO user_avatars (user_id, mime, bytes, updated_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(user_id) DO UPDATE SET mime = ?2, bytes = ?3, updated_at = ?4",
    )
    .bind(user_id)
    .bind(mime)
    .bind(bytes)
    .bind(now_unix())
    .execute(pool)
    .await?;
    Ok(())
}

/// Remove a user's avatar. A no-op when they never had one.
pub async fn delete_user_avatar(pool: &SqlitePool, user_id: i64) -> AuthResult<()> {
    sqlx::query("DELETE FROM user_avatars WHERE user_id = ?")
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
