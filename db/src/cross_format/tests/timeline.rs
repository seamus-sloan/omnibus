//! The linear mapping tier: concatenating a sequence link's audio files
//! into one timeline, and the percent ↔ seconds mapping (with inverse
//! consistency) that tier answers with before any chapter-anchoring runs.

use omnibus_shared::cross_format::CrossFormatLinkMode;

use crate::init_db;

use super::super::*;
use super::{seed_dual_book, seed_user};

#[tokio::test]
async fn audio_timeline_concatenates_sequence_files_by_ordinal() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();

    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tl.total_seconds, 1_000.0);
    assert_eq!(
        tl.files
            .iter()
            .map(|f| (f.book_file_id, f.start_seconds))
            .collect::<Vec<_>>(),
        vec![(audio[0], 0.0), (audio[1], 600.0), (audio[2], 900.0)]
    );
}

#[tokio::test]
async fn map_percent_to_audio_lands_in_the_right_file_and_inverts() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 300.0, 100.0]).await;
    let link = upsert_link(&pool, user, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(map_percent_to_audio(&tl, 0), Some((audio[0], 0.0)));
    assert_eq!(map_percent_to_audio(&tl, 65), Some((audio[1], 50.0)));
    assert_eq!(map_percent_to_audio(&tl, 95), Some((audio[2], 50.0)));
    // 100% belongs to the last file's end, never past it.
    assert_eq!(map_percent_to_audio(&tl, 100), Some((audio[2], 100.0)));
    assert_eq!(map_percent_to_audio(&tl, 101), None);

    // Inverse consistency within the floor's 1% granularity.
    for pct in [0i64, 13, 42, 65, 95, 100] {
        let (file, secs) = map_percent_to_audio(&tl, pct).unwrap();
        let back = map_audio_to_percent(&tl, file, secs).unwrap();
        assert!(
            (back - pct).abs() <= 1,
            "round trip drifted: {pct} -> {back}"
        );
    }
}

#[tokio::test]
async fn narrations_timeline_aligns_only_the_primary() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid, audio) = seed_dual_book(&pool, &[600.0, 620.0]).await;
    let link = upsert_link(
        &pool,
        user,
        &uuid,
        CrossFormatLinkMode::Narrations,
        Some(audio[1]),
    )
    .await
    .unwrap();

    let tl = audio_timeline(&pool, book_id, &link)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(tl.files.len(), 1);
    assert_eq!(tl.total_seconds, 620.0);
    assert_eq!(tl.files[0].book_file_id, audio[1]);
    // The non-primary narration is deliberately unaligned.
    assert_eq!(map_audio_to_percent(&tl, audio[0], 100.0), None);
}

#[test]
fn map_audio_to_percent_refuses_a_zero_length_timeline() {
    let tl = AudioTimeline {
        files: vec![TimelineFile {
            book_file_id: 1,
            start_seconds: 0.0,
            duration_seconds: 0.0,
        }],
        total_seconds: 0.0,
    };
    assert_eq!(map_audio_to_percent(&tl, 1, 0.0), None);
}
