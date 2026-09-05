//! `declare_sync_point` mechanics apart from CFI precision (covered in
//! `cfi`): auto-confirming a single-file link with follow on, leaving an
//! existing link's follow setting alone, the three ways a declaration can
//! be refused, and the accumulate-vs-replace rule for anchors declared
//! close together.

use omnibus_shared::cross_format::{CrossFormatLinkMode, DeclareSyncPoint};
use omnibus_shared::ProgressFormat;

use crate::init_db;

use super::super::*;
use super::{declare_audio, epub_percent_update, seed_dual_book, seed_user};

#[tokio::test]
async fn declare_sync_point_records_the_anchor_and_turns_follow_on() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Single audio file: the declaration may auto-confirm a sequence link.
    let (_, uuid, _) = seed_dual_book(&pool, &[1_000.0]).await;
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();

    let link = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap();
    assert!(link.follow);
    assert_eq!(link.user_anchors.len(), 1);
    let (t, s) = link.user_anchors[0];
    assert!((t - 0.40).abs() < 1e-9);
    assert!((s - 500.0).abs() < 1e-9);

    // The anchor bends the mapping: reading at 40% now maps to 500s
    // (linear would say 400s), and the resume carries follow + the
    // user-anchored confidence.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 2_000))
        .await
        .unwrap();
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert!(r.follow);
    let c = r.candidate.unwrap();
    assert_eq!(
        c.confidence,
        omnibus_shared::cross_format::MappingConfidence::UserAnchored
    );
    let secs = c.audio_position_seconds.unwrap();
    assert!(
        (495.0..=505.0).contains(&secs),
        "user anchor must pin 40% to ≈500s, got {secs}"
    );

    // Re-declaring nearby replaces the pair instead of stacking.
    let link = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 520.0))
        .await
        .unwrap();
    assert_eq!(link.user_anchors.len(), 1);
    assert!((link.user_anchors[0].1 - 520.0).abs() < 1e-9);
}

#[tokio::test]
async fn declare_sync_point_keeps_follow_off_on_a_link_that_already_existed() {
    // The off-switch would be cosmetic if a later "Sync here" flipped it
    // back on: a declaration is calibration, not a mode switch (#2155).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(set_follow(&pool, user, &uuid, false).await.unwrap());
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();

    let link = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap();
    assert!(!link.follow, "a declaration must not re-arm follow");
    // The anchor still lands — calibration is recorded either way.
    assert_eq!(link.user_anchors.len(), 1);

    // And the read side agrees, so nothing downstream silently re-follows.
    let r = resume_candidate(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap();
    assert!(!r.follow);
}

#[tokio::test]
async fn confirming_the_alignment_deliberately_re_arms_follow_the_reader_turned_off() {
    // The asymmetry with `declare_sync_point` above, pinned so it stays a
    // decision rather than an accident: a declaration is calibration and
    // leaves follow alone, but a confirm IS the act of turning sync on —
    // the button says so — and re-arms it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, _) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(set_follow(&pool, user, &uuid, false).await.unwrap());

    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    assert!(
        link.follow,
        "confirming the alignment is how sync is turned on; it re-arms follow"
    );
}

#[tokio::test]
async fn declare_sync_point_refuses_what_it_cannot_pair() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[600.0, 400.0]).await;

    // Multi-file with no link: never guess an order.
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::LinkRequired));

    // Linked, but the counterpart has no reading position to pair with.
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::CounterpartMissing));

    // A stale audio set pauses declarations exactly like offers.
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 10, 1_000))
        .await
        .unwrap();
    sqlx::query("UPDATE book_files SET mtime_epoch = 4242 WHERE id = ?")
        .bind(audio[0])
        .execute(&pool)
        .await
        .unwrap();
    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, Some(audio[0]), 100.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::AudioSetMismatch));
}

#[tokio::test]
async fn declare_sync_point_leaves_no_auto_created_link_when_the_pair_cannot_be_resolved() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Single audio file reaches the auto-confirm branch; nothing read yet
    // leaves it with no counterpart to pair.
    let (_, uuid, _) = seed_dual_book(&pool, &[1_000.0]).await;
    assert!(get_link(&pool, user, &uuid).await.unwrap().is_none());

    let err = declare_sync_point(&pool, user, &declare_audio(&uuid, None, 500.0))
        .await
        .unwrap_err();
    assert!(matches!(err, CrossFormatError::CounterpartMissing));

    // A link left behind would follow with no anchor the user declared.
    assert!(get_link(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn declare_sync_point_accumulates_nearby_anchors() {
    // Two declarations 1% of the book apart must BOTH survive — on a
    // 50-hour timeline the old 2% replace slack silently discarded
    // re-syncs an hour apart (#1954).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid, audio) = seed_dual_book(&pool, &[1_000.0]).await;
    upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 40, 1_000))
        .await
        .unwrap();
    declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            ebook_fraction: None,
            epub_cfi: None,
            audio_book_file_id: Some(audio[0]),
            audio_seconds: Some(400.0),
        },
    )
    .await
    .unwrap();
    progress::upsert_progress(&pool, user, &epub_percent_update(&uuid, 41, 2_000))
        .await
        .unwrap();
    let link = declare_sync_point(
        &pool,
        user,
        &DeclareSyncPoint {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            ebook_fraction: None,
            epub_cfi: None,
            audio_book_file_id: Some(audio[0]),
            audio_seconds: Some(410.0),
        },
    )
    .await
    .unwrap();
    assert_eq!(
        link.user_anchors.len(),
        2,
        "a re-sync 1% away must accumulate, not replace: {:?}",
        link.user_anchors
    );
}
