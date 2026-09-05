//! Per-reader state across a merge: progress and playback rate with
//! latest-wins on collision, highlights, the Kobo annotation sync state
//! preferring the target's row, and read status / ratings / journals with
//! the newer row kept.

use super::super::*;
use super::seed_user;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, AudiobookSyncPlan};
use crate::test_support::{
    count_rows as count, indexed_audiobook, seed_synced_audiobook as seed_audiobook,
    seed_synced_ebook as seed_ebook,
};

#[tokio::test]
async fn merge_books_moves_progress_with_latest_wins_on_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    // M4B + MP3: passes the file-format collision check but both map to
    // the coarse 'audio' progress format.
    let target = seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    let mut mp3 = indexed_audiobook("B/Drakula mp3", "Drakula", Some("Bram Stoker"));
    mp3.format = "MP3".into();
    let source_scan_key = mp3.scan_key.clone();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Identity is minted (F2) — read the durable uuid back by scan_key.
    let source = crate::test_support::uuid_by_scan_key(&pool, &source_scan_key).await;

    // User-data tables soft-ref the durable `books.uuid` (F1).
    for (book_uuid, pos, ts) in [(&target, 100.0, 1000), (&source, 200.0, 2000)] {
        sqlx::query(
            "INSERT INTO reading_progress (user_id, book_uuid, format, audio_position_seconds, updated_at)
             VALUES (?, ?, 'audio', ?, ?)",
        )
        .bind(user)
        .bind(book_uuid)
        .bind(pos)
        .bind(ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    // Exactly one row survives, re-parented to the target: the newer
    // (source's) position.
    let rows: Vec<(String, f64)> =
        sqlx::query_as("SELECT book_uuid, audio_position_seconds FROM reading_progress")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(target.clone(), 200.0)]);
}

#[tokio::test]
async fn merge_books_moves_playback_rate_with_latest_wins_on_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    let mut mp3 = indexed_audiobook("B/Drakula mp3", "Drakula", Some("Bram Stoker"));
    mp3.format = "MP3".into();
    let source_scan_key = mp3.scan_key.clone();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = crate::test_support::uuid_by_scan_key(&pool, &source_scan_key).await;

    for (book_uuid, playback_rate, updated_at) in [(&target, 1.25, 1000), (&source, 1.75, 2000)] {
        sqlx::query(
            "INSERT INTO audiobook_playback_preferences
                (user_id, book_uuid, playback_rate, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(user)
        .bind(book_uuid)
        .bind(playback_rate)
        .bind(updated_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let rows: Vec<(String, f64)> =
        sqlx::query_as("SELECT book_uuid, playback_rate FROM audiobook_playback_preferences")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(target.clone(), 1.75)]);
}

#[tokio::test]
async fn merge_moves_highlights_to_target() {
    // F1 merge fix: highlights are now in the re-parent set, so a manual
    // merge no longer loses the source book's highlights.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    crate::annotations::create_highlight(
        &pool,
        user,
        &omnibus_shared::CreateHighlight {
            client_id: None,
            book_uuid: source.clone(),
            epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:100)".into(),
            color: omnibus_shared::HighlightColor::Blue,
            text: None,
        },
    )
    .await
    .unwrap();

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let on_target = crate::annotations::list_highlights(&pool, user, &target)
        .await
        .unwrap();
    assert_eq!(
        on_target.len(),
        1,
        "the source book's highlight must re-parent to the target on merge"
    );
}

#[tokio::test]
async fn merge_retargets_kobo_annotation_sync_state_preferring_the_targets_row() {
    // #1278: the per-device annotation watermark follows the merge. Where a
    // device tracks both uuids the target's row wins (PK collision); a
    // device that only tracked the source keeps its adoption under the
    // target's uuid.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    let both = crate::kobo_devices::create_device(&pool, user, "Tracks both")
        .await
        .unwrap();
    let source_only = crate::kobo_devices::create_device(&pool, user, "Tracks source")
        .await
        .unwrap();
    // ack_served (#1647) only sticks for a book the device is on record as
    // holding — seed that first so these rows exist to retarget.
    crate::kobo::annotations::mark_downloaded(&pool, both.id, &target)
        .await
        .unwrap();
    crate::kobo::annotations::mark_downloaded(&pool, both.id, &source)
        .await
        .unwrap();
    crate::kobo::annotations::ack_served(&pool, both.id, &target, "fp-target")
        .await
        .unwrap();
    crate::kobo::annotations::ack_served(&pool, both.id, &source, "fp-source")
        .await
        .unwrap();
    crate::kobo::annotations::mark_adopted(&pool, source_only.id, &source)
        .await
        .unwrap();

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let stale: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kobo_annotations_sync WHERE book_uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stale, 0,
        "no watermark may keep pointing at the source uuid"
    );

    let kept_ack: String = sqlx::query_scalar(
        "SELECT acked_fingerprint FROM kobo_annotations_sync
          WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(both.id)
    .bind(&target)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(kept_ack, "fp-target", "the target's row wins the collision");

    assert!(
        crate::kobo::annotations::is_adopted(&pool, source_only.id, &target)
            .await
            .unwrap(),
        "a source-only device's adoption must follow the merge"
    );
}

/// Stamp a read status, a rating and a journal entry on `book_uuid`.
async fn seed_reader_record(
    pool: &sqlx::SqlitePool,
    user: i64,
    book_uuid: &str,
    half_stars: i64,
    body: &str,
    ts: i64,
) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user)
    .bind(book_uuid)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(user)
    .bind(book_uuid)
    .bind(half_stars)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at, updated_at)
         VALUES (?, ?, ?, 100, ?, ?)",
    )
    .bind(user)
    .bind(book_uuid)
    .bind(body)
    .bind(ts)
    .bind(ts)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn merge_books_moves_read_status_ratings_and_journals_to_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "Stoker/Drakula.m4b", "Drakula", "Bram Stoker").await;

    // Only the book about to be merged away carries the reader's record.
    seed_reader_record(&pool, user, &source, 10, "what a book", 2_000).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    // All three follow the book rather than stranding on the deleted uuid.
    let status: Vec<(String, String)> =
        sqlx::query_as("SELECT book_uuid, status FROM book_read_status")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(status, vec![(target.clone(), "finished".to_string())]);

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![(target.clone(), 10)]);

    let journals: Vec<(String, String)> =
        sqlx::query_as("SELECT book_uuid, body_md FROM journal_entries")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(journals, vec![(target.clone(), "what a book".to_string())]);
}

#[tokio::test]
async fn merge_books_keeps_the_newer_read_status_and_rating_on_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "Stoker/Drakula.m4b", "Drakula", "Bram Stoker").await;

    // Both sides rated and finished; the source's record is the newer one.
    seed_reader_record(&pool, user, &target, 6, "the ebook", 1_000).await;
    seed_reader_record(&pool, user, &source, 10, "the audiobook", 2_000).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    // One row each, on the target, carrying the newer side's values.
    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![(target.clone(), 10)]);

    let finished: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, finished_at FROM book_read_status")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(finished, vec![(target.clone(), 2_000)]);

    // Journals have no per-book uniqueness, so both entries survive the move.
    let journals: i64 = count(
        &pool,
        &format!("SELECT COUNT(*) FROM journal_entries WHERE book_uuid = '{target}'"),
    )
    .await;
    assert_eq!(journals, 2);
}

#[tokio::test]
async fn merge_books_keeps_the_target_rating_when_it_is_the_newer_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "Stoker/Drakula.m4b", "Drakula", "Bram Stoker").await;

    // Mirror of the test above — this time the surviving book is newer.
    seed_reader_record(&pool, user, &target, 6, "the ebook", 3_000).await;
    seed_reader_record(&pool, user, &source, 10, "the audiobook", 2_000).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let ratings: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, half_stars FROM user_ratings")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(ratings, vec![(target.clone(), 6)]);
}
