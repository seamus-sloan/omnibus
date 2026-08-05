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
