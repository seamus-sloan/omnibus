//! Link CRUD, staleness, the audio-file ordering override, and the
//! reconfirm/reorder rules for when a stored link's `follow` flag and user
//! anchors survive versus get cleared.

use omnibus_shared::cross_format::CrossFormatLinkMode;

use crate::init_db;

use super::super::*;
use super::{declare_audio, epub_percent_update, seed_dual_book, seed_user};

#[tokio::test]
async fn upsert_get_delete_link_round_trips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0]).await;

    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert_eq!(link.mode, CrossFormatLinkMode::Sequence);
    assert!(link.audio_snapshot.contains("a0.m4b"));
    assert!(link.confirmed_at > 0);

    // Re-confirming as narrations replaces the row wholesale.
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[0]),
    )
    .await
    .unwrap();
    assert_eq!(link.mode, CrossFormatLinkMode::Narrations);
    assert_eq!(link.primary_book_file_id, Some(audio[0]));

    assert!(delete_link(&pool, user, &uuid).await.unwrap());
    assert!(!delete_link(&pool, user, &uuid).await.unwrap());
    assert!(get_link(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn upsert_link_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let err = upsert_link(&pool, user, "no-such", CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::BookNotFound));
}

#[tokio::test]
async fn get_link_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_link(&pool, 1, "some-uuid").await;
    assert!(matches!(err, Err(CrossFormatError::Sqlx(_))));
}

#[tokio::test]
async fn link_is_stale_flips_when_the_audio_file_set_changes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(!link_is_stale(&pool, book_id, &link).await.unwrap());

    // A replaced file moves its wire etag (mtime/size) → stale.
    sqlx::query("UPDATE book_files SET mtime_epoch = 9999 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    assert!(link_is_stale(&pool, book_id, &link).await.unwrap());
}

#[tokio::test]
async fn set_audio_order_persists_and_refuses_a_mismatched_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;

    set_audio_order(&pool, &uuid, &[audio[1], audio[0]])
        .await
        .unwrap();
    let view = alignment_view(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        view.audio_files
            .iter()
            .map(|f| f.book_file_id)
            .collect::<Vec<_>>(),
        vec![audio[1], audio[0]]
    );

    // Anything that isn't exactly the current set refuses with the
    // dedicated mismatch variant: missing file, foreign id, duplicate.
    for bad in [
        vec![audio[0]],
        vec![audio[0], 999_999],
        vec![audio[0], audio[0]],
    ] {
        assert!(matches!(
            set_audio_order(&pool, &uuid, &bad).await.unwrap_err(),
            CrossFormatError::AudioSetMismatch
        ));
    }
}

#[tokio::test]
async fn set_follow_flips_only_existing_links() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[600.0]).await;
    assert!(!set_follow(&pool, user, &uuid, true).await.unwrap());
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(set_follow(&pool, user, &uuid, true).await.unwrap());
    assert!(get_link(&pool, user, &uuid).await.unwrap().unwrap().follow);
    assert!(set_follow(&pool, user, &uuid, false).await.unwrap());
    assert!(!get_link(&pool, user, &uuid).await.unwrap().unwrap().follow);
}

#[tokio::test]
async fn reconfirm_keeps_follow_and_clears_anchors_only_when_the_audio_set_changed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap();

    // Same audio set: re-confirming keeps follow and the anchors.
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.follow);
    assert_eq!(link.user_anchors.len(), 1);

    // Changed audio set: anchors' global seconds mean nothing on the new
    // timeline — cleared; the follow opt-in survives.
    sqlx::query("UPDATE book_files SET mtime_epoch = 777 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.follow);
    assert!(link.user_anchors.is_empty());
}

#[tokio::test]
async fn reordering_audio_files_reads_as_a_different_set_and_clears_anchors() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 500.0))
        .await
        .unwrap();

    // Swap the two files' ordinals — the concatenated timeline changes,
    // so every stored global second now means a different spot. The
    // snapshot must read as a different set: the link goes stale for
    // everyone, and a re-confirm clears the old-ruler anchors.
    // Three steps through a parking ordinal: the UNIQUE(book_id, format,
    // ordinal) constraint is enforced per-row, so a single-statement swap
    // trips it mid-UPDATE.
    for (id, ordinal) in [(audio[0], 99), (audio[1], 0), (audio[0], 1)] {
        sqlx::query("UPDATE book_files SET ordinal = ? WHERE id = ?")
            .bind(ordinal)
            .bind(id)
            .execute(&pool)
            .await
            .unwrap();
    }
    let book_id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();
    let link = get_link(&pool, user, &uuid).await.unwrap().unwrap();
    assert!(link_is_stale(&pool, book_id, &link).await.unwrap());

    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(link.user_anchors.is_empty());
}

#[tokio::test]
async fn reconfirm_with_a_different_mode_or_primary_clears_anchors() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 500.0))
        .await
        .unwrap();

    // Same files, different mode: the anchors' global seconds were
    // measured on the sequence concatenation — meaningless against a
    // narrations primary. Cleared.
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[0]),
    )
    .await
    .unwrap();
    assert!(link.user_anchors.is_empty());

    // Re-anchor on the narrations primary, then switch primaries: a
    // different recording is a different ruler too. Cleared again.
    declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap();
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[1]),
    )
    .await
    .unwrap();
    assert!(link.user_anchors.is_empty());
}
