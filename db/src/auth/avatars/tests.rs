//! Tests for per-user avatar storage: roundtrip, replace-on-upload, delete,
//! and the `has_avatar` flag every user read derives from these rows.

use super::*;
use crate::auth::{create_user, get_user_by_id, test_support::pool};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nfake-bytes";
const JPEG: &[u8] = b"\xff\xd8\xff\xe0fake-bytes";

#[tokio::test]
async fn get_user_avatar_returns_none_when_none_was_uploaded() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert_eq!(get_user_avatar(&p, u.id).await.unwrap(), None);
}

#[tokio::test]
async fn upsert_user_avatar_roundtrips_mime_and_bytes() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    upsert_user_avatar(&p, u.id, "image/png", PNG)
        .await
        .unwrap();

    let stored = get_user_avatar(&p, u.id).await.unwrap().unwrap();
    assert_eq!(stored.mime, "image/png");
    assert_eq!(stored.bytes, PNG);
}

#[tokio::test]
async fn upsert_user_avatar_replaces_the_previous_image() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    upsert_user_avatar(&p, u.id, "image/png", PNG)
        .await
        .unwrap();
    upsert_user_avatar(&p, u.id, "image/jpeg", JPEG)
        .await
        .unwrap();

    let stored = get_user_avatar(&p, u.id).await.unwrap().unwrap();
    assert_eq!(stored.mime, "image/jpeg");
    assert_eq!(stored.bytes, JPEG);
    // Replace, never accumulate — one row per user is the whole contract.
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_avatars WHERE user_id = ?")
        .bind(u.id)
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn delete_user_avatar_removes_the_row_and_is_idempotent() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    upsert_user_avatar(&p, u.id, "image/png", PNG)
        .await
        .unwrap();

    delete_user_avatar(&p, u.id).await.unwrap();
    assert_eq!(get_user_avatar(&p, u.id).await.unwrap(), None);

    // Clearing an absent avatar is not an error.
    delete_user_avatar(&p, u.id).await.unwrap();
}

#[tokio::test]
async fn user_reads_derive_has_avatar_from_the_stored_row() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert!(!u.has_avatar);
    assert!(!get_user_by_id(&p, u.id).await.unwrap().unwrap().has_avatar);

    upsert_user_avatar(&p, u.id, "image/png", PNG)
        .await
        .unwrap();
    assert!(get_user_by_id(&p, u.id).await.unwrap().unwrap().has_avatar);

    delete_user_avatar(&p, u.id).await.unwrap();
    assert!(!get_user_by_id(&p, u.id).await.unwrap().unwrap().has_avatar);
}

/// A real (decodable) PNG of `w` x `h`, so the thumbnail path has something
/// to downscale — the `PNG` constant above is deliberately undecodable.
fn real_png(w: u32, h: u32) -> Vec<u8> {
    crate::test_support::solid_color_png(20, 120, 200, w, h)
}

/// AC1/AC2 of #2245: the nav is served the downscaled rendering, and a large
/// upload does not become a large per-page fetch.
#[tokio::test]
async fn upsert_user_avatar_stores_a_thumbnail_the_default_read_serves() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let original = real_png(1200, 1800);

    upsert_user_avatar(&p, u.id, "image/png", &original)
        .await
        .unwrap();

    let thumb = get_user_avatar_variant(&p, u.id, AvatarVariant::Thumb)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thumb.mime, "image/webp");
    assert!(
        thumb.bytes.len() < original.len(),
        "thumbnail ({} bytes) must be smaller than the {} byte upload",
        thumb.bytes.len(),
        original.len()
    );
    let decoded = image::load_from_memory(&thumb.bytes).expect("thumbnail decodes");
    assert!(decoded.width().max(decoded.height()) <= 160);
    // Aspect preserved rather than cropped: 2:3 in, 2:3 out (within the
    // rounding `resize` does on the short edge).
    assert_eq!(decoded.height(), 160);
    assert!(
        (106..=108).contains(&decoded.width()),
        "expected ~107px wide, got {}",
        decoded.width()
    );

    // The original is kept for any surface that wants a larger rendering.
    let full = get_user_avatar(&p, u.id).await.unwrap().unwrap();
    assert_eq!(full.mime, "image/png");
    assert_eq!(full.bytes, original);
}

/// Bytes no decoder accepts still store — an avatar that renders large beats
/// one that doesn't render — and the thumb read falls back to the original.
#[tokio::test]
async fn get_user_avatar_variant_falls_back_to_the_original_without_a_thumbnail() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    upsert_user_avatar(&p, u.id, "image/png", PNG)
        .await
        .unwrap();

    let thumb = get_user_avatar_variant(&p, u.id, AvatarVariant::Thumb)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thumb.mime, "image/png");
    assert_eq!(thumb.bytes, PNG);
}

/// An avatar uploaded before migration 0084 has no thumbnail until the boot
/// backfill derives one — and the pass converges rather than re-encoding
/// every boot.
#[tokio::test]
async fn backfill_avatar_thumbs_fills_a_row_uploaded_before_the_column_existed() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    upsert_user_avatar(&p, u.id, "image/png", &real_png(600, 600))
        .await
        .unwrap();
    // Back to the pre-migration shape.
    sqlx::query("UPDATE user_avatars SET thumb_mime = NULL, thumb_bytes = NULL")
        .execute(&p)
        .await
        .unwrap();

    backfill_avatar_thumbs(&p).await.unwrap();

    let thumb = get_user_avatar_variant(&p, u.id, AvatarVariant::Thumb)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(thumb.mime, "image/webp");

    let pending: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM user_avatars WHERE thumb_bytes IS NULL")
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(pending, 0);
}
