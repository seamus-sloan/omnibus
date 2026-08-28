//! Per-user avatar bytes: one row per user, replaced in place on upload.
//!
//! Stored in the DB rather than on disk because an avatar is small, has exactly
//! one owner, and is always replaced rather than accumulated — an upsert covers
//! the whole lifecycle with no orphan sweep and no extra directory to configure.
//!
//! Each row carries two renderings: the original upload and a downscaled WebP
//! thumbnail derived from it at write time. Every surface that draws an avatar
//! today is a small circle, so the thumbnail is what the endpoint serves; the
//! original is kept for anything that later wants it full size.

use sqlx::{Row, SqlitePool};

use super::{now_unix, AuthResult};

/// Longest edge of the stored avatar thumbnail, in pixels.
///
/// The largest avatar circle in either client is 80px (`.disc-avatar`), so 160
/// covers it at 2x device pixel ratio with nothing to spare — a 28px nav square
/// was previously being filled with a 1446x2200 original (#2245).
const THUMB_MAX_EDGE: u32 = 160;

/// Lossy WebP quality on libwebp's 0-100 scale, matching `thumbs::THUMB_QUALITY`.
const THUMB_QUALITY: f32 = 80.0;

/// The MIME the derived thumbnail is always stored and served as.
const THUMB_MIME: &str = "image/webp";

/// A stored avatar: its content type and the image bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserAvatar {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// Which rendering of an avatar to read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarVariant {
    /// The downscaled WebP, falling back to the original when a row predates
    /// the thumbnail column or holds bytes that wouldn't decode.
    Thumb,
    /// The upload exactly as it arrived.
    Full,
}

/// Read a user's avatar in its original size, or `None` when they haven't set
/// one.
pub async fn get_user_avatar(pool: &SqlitePool, user_id: i64) -> AuthResult<Option<UserAvatar>> {
    get_user_avatar_variant(pool, user_id, AvatarVariant::Full).await
}

/// Read one rendering of a user's avatar, or `None` when they haven't set one.
///
/// [`AvatarVariant::Thumb`] falls back to the original rather than returning
/// `None`: a row can legitimately carry no thumbnail (uploaded before the
/// column existed and not yet backfilled, or bytes no decoder accepts), and an
/// avatar that renders large is better than one that doesn't render.
pub async fn get_user_avatar_variant(
    pool: &SqlitePool,
    user_id: i64,
    variant: AvatarVariant,
) -> AuthResult<Option<UserAvatar>> {
    let row = sqlx::query(
        "SELECT mime, bytes, thumb_mime, thumb_bytes FROM user_avatars WHERE user_id = ?",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| {
        let thumb = match variant {
            AvatarVariant::Thumb => r
                .get::<Option<String>, _>("thumb_mime")
                .zip(r.get::<Option<Vec<u8>>, _>("thumb_bytes")),
            AvatarVariant::Full => None,
        };
        match thumb {
            Some((mime, bytes)) => UserAvatar { mime, bytes },
            None => UserAvatar {
                mime: r.get("mime"),
                bytes: r.get("bytes"),
            },
        }
    }))
}

/// Store a user's avatar, replacing any existing one, and derive its
/// thumbnail in the same write.
///
/// `mime` must be the *sniffed* type from `image_upload::extract_validated_image`,
/// not the client's header — it is echoed back as the `Content-Type` on every
/// later read of the original.
pub async fn upsert_user_avatar(
    pool: &SqlitePool,
    user_id: i64,
    mime: &str,
    bytes: &[u8],
) -> AuthResult<()> {
    let thumb = derive_thumb(bytes.to_vec()).await;
    sqlx::query(
        "INSERT INTO user_avatars (user_id, mime, bytes, thumb_mime, thumb_bytes, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
         ON CONFLICT(user_id) DO UPDATE SET \
             mime = ?2, bytes = ?3, thumb_mime = ?4, thumb_bytes = ?5, updated_at = ?6",
    )
    .bind(user_id)
    .bind(mime)
    .bind(bytes)
    .bind(thumb.as_ref().map(|_| THUMB_MIME))
    .bind(thumb)
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

/// Fill `thumb_bytes` for avatars uploaded before the column existed
/// (migration `0086`).
///
/// Boot backfill, and a no-op once every row either has a thumbnail or has
/// been shown not to produce one. Bytes no decoder accepts stay NULL and are
/// retried on the next boot — the endpoint keeps serving the original for
/// them, so nothing is broken by the retry, and the alternative (a
/// "tried and failed" sentinel column) is more schema than the case is worth.
pub async fn backfill_avatar_thumbs(pool: &SqlitePool) -> AuthResult<()> {
    let rows: Vec<(i64, Vec<u8>)> =
        sqlx::query_as("SELECT user_id, bytes FROM user_avatars WHERE thumb_bytes IS NULL")
            .fetch_all(pool)
            .await?;
    if rows.is_empty() {
        return Ok(());
    }
    let mut filled = 0usize;
    for (user_id, bytes) in rows {
        let Some(thumb) = derive_thumb(bytes).await else {
            continue;
        };
        sqlx::query("UPDATE user_avatars SET thumb_mime = ?, thumb_bytes = ? WHERE user_id = ?")
            .bind(THUMB_MIME)
            .bind(thumb)
            .bind(user_id)
            .execute(pool)
            .await?;
        filled += 1;
    }
    if filled > 0 {
        tracing::info!(count = filled, "boot backfill: derived avatar thumbnails");
    }
    Ok(())
}

/// Downscale `bytes` to a WebP thumbnail, or `None` when they don't decode.
/// Decode + encode are CPU-bound, so the work runs on the blocking pool.
async fn derive_thumb(bytes: Vec<u8>) -> Option<Vec<u8>> {
    tokio::task::spawn_blocking(move || downscale_avatar(&bytes))
        .await
        .unwrap_or_else(|join_err| {
            tracing::warn!(
                %join_err,
                is_panic = join_err.is_panic(),
                "avatar thumbnail task failed; storing the original only"
            );
            None
        })
}

/// Resize to fit inside [`THUMB_MAX_EDGE`] and encode as lossy WebP.
///
/// `resize` rather than `resize_to_fill`: an avatar is drawn inside a circle
/// the client already crops with `object-fit: cover`, so cropping here would
/// only throw away pixels twice. An image already inside the box is still
/// re-encoded — a 3 MB PNG at 100x100 is exactly the payload this exists to
/// avoid.
fn downscale_avatar(bytes: &[u8]) -> Option<Vec<u8>> {
    use image::imageops::FilterType;

    let decoded = image::load_from_memory(bytes).ok()?;
    let resized = decoded.resize(THUMB_MAX_EDGE, THUMB_MAX_EDGE, FilterType::Lanczos3);
    let rgba = resized.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let encoded = webp::Encoder::from_rgba(rgba.as_raw(), w, h)
        .encode_simple(false, THUMB_QUALITY)
        .ok()?
        .to_vec();
    (!encoded.is_empty()).then_some(encoded)
}

#[cfg(test)]
mod tests;
