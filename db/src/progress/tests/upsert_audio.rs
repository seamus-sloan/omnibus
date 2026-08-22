//! The audio side of the `upsert_progress` write path: the multi-file guard
//! that refuses a fileless write on a multi-file book, the stored
//! `book_file_id`, and the near-zero teardown refusal.

use omnibus_shared::ProgressUpdate;

use crate::init_db;

use super::super::*;
use super::{
    audio_update, seed, seed_audio_files, seed_audiobook, seed_second_audiobook_file, seed_user,
};

#[tokio::test]
async fn upsert_rejects_fileless_audio_write_that_would_blank_a_named_file_on_multi_file_book() {
    // #1888: a web player that lost track of which file it loaded must not
    // be able to replace "23,718 s within file 4" with "23,718 s within an
    // unknown file". The whole write is rejected — seconds included — and
    // the surviving row is returned, mirroring a stale-clock rejection.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 2).await;

    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 23_718.0, Some(files[1]), 100),
    )
    .await
    .unwrap();
    let survived = upsert_progress(&pool, user, &audio_update(&uuid, 60.0, None, 200))
        .await
        .unwrap();

    assert_eq!(survived.book_file_id, Some(files[1]));
    assert_eq!(survived.audio_position_seconds, Some(23_718.0));
    let stored = get_progress(&pool, user, &uuid, ProgressFormat::Audio)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.book_file_id, Some(files[1]));
    assert_eq!(stored.audio_position_seconds, Some(23_718.0));
    assert_eq!(stored.client_updated_at, 100);
}

#[tokio::test]
async fn upsert_applies_fileless_audio_write_on_a_single_file_book() {
    // Legacy single-file behavior is untouched: with one audio file there
    // is nothing ambiguous about a fileless write, so it still replaces
    // the whole row (file id included).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 1).await;

    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 500.0, Some(files[0]), 100),
    )
    .await
    .unwrap();
    let record = upsert_progress(&pool, user, &audio_update(&uuid, 600.0, None, 200))
        .await
        .unwrap();

    assert_eq!(record.audio_position_seconds, Some(600.0));
    assert_eq!(record.book_file_id, None);
}

#[tokio::test]
async fn upsert_applies_multi_file_audio_write_that_names_its_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let files = seed_audio_files(&pool, book_id, 2).await;

    upsert_progress(
        &pool,
        user,
        &audio_update(&uuid, 23_718.0, Some(files[1]), 100),
    )
    .await
    .unwrap();
    let record = upsert_progress(&pool, user, &audio_update(&uuid, 30.0, Some(files[0]), 200))
        .await
        .unwrap();

    assert_eq!(record.book_file_id, Some(files[0]));
    assert_eq!(record.audio_position_seconds, Some(30.0));
}

#[tokio::test]
async fn upsert_round_trips_the_audio_file_the_position_was_taken_in() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id = seed_audiobook(&pool, uuid).await;
    let second = seed_second_audiobook_file(&pool, book_id).await;

    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(90.0),
            book_file_id: Some(second),
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(saved.book_file_id, Some(second));

    let fetched = get_progress(&pool, user, uuid, ProgressFormat::Audio)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.book_file_id, Some(second));
}

#[tokio::test]
async fn upsert_progress_refuses_near_zero_audio_write_against_far_advanced_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Guarded").await;
    upsert_progress(&pool, user, &audio_update(&uuid, 10_000.0, None, 1_000))
        .await
        .unwrap();

    // The teardown signature: ~0 with a fresh clock. Dropped — the stored
    // row comes back untouched.
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 0.0, None, 2_000))
        .await
        .unwrap();
    assert!((rec.audio_position_seconds.unwrap() - 10_000.0).abs() < f64::EPSILON);

    // A genuine restart persists within one heartbeat: the next report is
    // already past the cutoff and lands normally.
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 5.0, None, 3_000))
        .await
        .unwrap();
    assert!((rec.audio_position_seconds.unwrap() - 5.0).abs() < f64::EPSILON);

    // A barely-started book restarts instantly — the refusal only guards
    // rows already far into the book.
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 0.0, None, 4_000))
        .await
        .unwrap();
    assert!(rec.audio_position_seconds.unwrap() < f64::EPSILON);
}

#[tokio::test]
async fn upsert_progress_teardown_refusal_exempts_a_write_in_a_different_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Switched").await;
    upsert_progress(&pool, user, &audio_update(&uuid, 10_000.0, Some(7), 1_000))
        .await
        .unwrap();

    // Near-zero, fresh clock — but naming a DIFFERENT file than the
    // stored row: that is a real file switch (picker, mapped offer), not
    // a dying element's flush, and refusing it would lose the switch.
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 0.5, Some(8), 2_000))
        .await
        .unwrap();
    assert_eq!(rec.book_file_id, Some(8));
    assert!((rec.audio_position_seconds.unwrap() - 0.5).abs() < f64::EPSILON);

    // Same-file near-zero stays refused.
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 10_000.0, Some(8), 3_000))
        .await
        .unwrap();
    assert!((rec.audio_position_seconds.unwrap() - 10_000.0).abs() < f64::EPSILON);
    let rec = upsert_progress(&pool, user, &audio_update(&uuid, 0.0, Some(8), 4_000))
        .await
        .unwrap();
    assert!((rec.audio_position_seconds.unwrap() - 10_000.0).abs() < f64::EPSILON);
}
