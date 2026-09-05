//! The forward-progress ledgers across a merge: day buckets and
//! quarter-hour slots are summed onto the target, the newer mark is kept,
//! and the sitting clock is unset only when the surviving mark could act
//! as a ceiling.

use super::super::*;
use super::seed_user;
use crate::pool::init_db;
use crate::test_support::{
    seed_synced_audiobook as seed_audiobook, seed_synced_ebook as seed_ebook,
};

/// Seed one forward-progress day bucket (migration 0083) for a book.
async fn seed_daily_ledger(pool: &sqlx::SqlitePool, user: i64, uuid: &str, day: &str, gained: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily
             (user_id, book_uuid, format, day, percent_gained, updated_at)
         VALUES (?, ?, 'epub', ?, ?, 1)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(gained)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn merge_sums_forward_progress_day_buckets_onto_the_target() {
    // #2139: the day buckets are a counter, not a snapshot. Latest-wins — the
    // rule every other colliding table here follows — would throw away a whole
    // day of reading from one edition.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_daily_ledger(&pool, user, &target, "2026-08-01", 10).await;
    seed_daily_ledger(&pool, user, &source, "2026-08-01", 15).await;
    // A day only the source has must survive the retarget untouched.
    seed_daily_ledger(&pool, user, &source, "2026-08-02", 7).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT day, percent_gained FROM reading_progress_daily
          WHERE book_uuid = ? ORDER BY day",
    )
    .bind(&target)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            ("2026-08-01".to_string(), 25),
            ("2026-08-02".to_string(), 7)
        ]
    );
    // Nothing stranded on the deleted source uuid, where it would be invisible
    // to every query that joins `books`.
    let stranded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reading_progress_daily WHERE book_uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stranded, 0);
}

/// Seed one forward-progress quarter-hour slot (migration 0093) for a book.
async fn seed_slot_ledger(pool: &sqlx::SqlitePool, user: i64, uuid: &str, slot: i64, gained: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_slots
             (user_id, book_uuid, format, slot, percent_gained, updated_at)
         VALUES (?, ?, 'epub', ?, ?, 1)",
    )
    .bind(user)
    .bind(uuid)
    .bind(slot)
    .bind(gained)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn merge_sums_forward_progress_slots_onto_the_target() {
    // The slot table replaced the day buckets it sits beside, and inherits their
    // semantics exactly: it is a counter, so a collision sums. A per-reader table
    // left off `RETARGET_TABLES` strands its rows on the deleted source uuid,
    // invisible to every query that joins `books` — which is why the new
    // generation needs its own coverage rather than riding the old one's.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    seed_slot_ledger(&pool, user, &target, 1_000, 10).await;
    seed_slot_ledger(&pool, user, &source, 1_000, 15).await;
    // A slot only the source has must survive the retarget untouched.
    seed_slot_ledger(&pool, user, &source, 1_001, 7).await;

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let rows: Vec<(i64, i64)> = sqlx::query_as(
        "SELECT slot, percent_gained FROM reading_progress_slots
          WHERE book_uuid = ? ORDER BY slot",
    )
    .bind(&target)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(1_000, 25), (1_001, 7)]);

    let stranded: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM reading_progress_slots WHERE book_uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(stranded, 0);
}

#[tokio::test]
async fn merge_keeps_the_newer_forward_progress_mark() {
    // The mark shadows a position, so latest-wins is right here — unlike the
    // day buckets it sits beside.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    for (uuid, percent, ts) in [(&target, 20, 1000), (&source, 55, 2000)] {
        sqlx::query(
            "INSERT INTO reading_progress_marks
                 (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
             VALUES (?, ?, 'epub', ?, ?, ?)",
        )
        .bind(user)
        .bind(uuid)
        .bind(percent)
        .bind(ts)
        .bind(ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let marks: Vec<(String, i64)> =
        sqlx::query_as("SELECT book_uuid, sitting_max_percent FROM reading_progress_marks")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(marks, vec![(target.clone(), 55)]);
}

#[tokio::test]
async fn merge_unsets_the_sitting_clock_so_the_surviving_mark_cannot_act_as_a_ceiling() {
    // The surviving mark is picked by `updated_at`, so a source book read to
    // 90% can win over a target read to 10%. Since migration 0093 that mark is
    // a ceiling on accrual, so leaving its clock live would suppress every gain
    // under 90% on the merged book until the next idle gap — the reader reads
    // and the tile reports nothing. A NULL clock re-baselines instead.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    for (uuid, percent, ts) in [(&target, 10, 4000), (&source, 90, 5000)] {
        sqlx::query(
            "INSERT INTO reading_progress_marks
                 (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
             VALUES (?, ?, 'epub', ?, ?, ?)",
        )
        .bind(user)
        .bind(uuid)
        .bind(percent)
        .bind(ts)
        .bind(ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let clock: Option<i64> =
        sqlx::query_scalar("SELECT sitting_observed_at FROM reading_progress_marks")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(clock, None, "merge must leave no sitting in progress");
}

#[tokio::test]
async fn merge_leaves_the_sitting_clock_alone_when_the_two_marks_are_different_formats() {
    // The dedupe keys on `format`, so an epub mark on one book and an audio
    // mark on the other never collide — neither's provenance is in doubt and
    // neither should re-baseline. This is the *usual* merge, not an edge case:
    // merging an audiobook into an ebook is what `merge_books` is for.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    for (uuid, format) in [(&target, "epub"), (&source, "audio")] {
        sqlx::query(
            "INSERT INTO reading_progress_marks
                 (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
             VALUES (?, ?, ?, 50, 4000, 4000)",
        )
        .bind(user)
        .bind(uuid)
        .bind(format)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let clocks: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT format, sitting_observed_at FROM reading_progress_marks")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(clocks.len(), 2, "both marks survive — they never collided");
    assert!(
        clocks.iter().all(|(_, clock)| *clock == Some(4000)),
        "neither sitting should have been reset, got {clocks:?}"
    );
}

#[tokio::test]
async fn merge_leaves_the_sitting_clock_alone_for_a_reader_holding_only_one_of_the_books() {
    // The clear is scoped to the readers the dedupe actually chooses between.
    // This reader has a mark on the target only, so nothing about their mark
    // changes — re-baselining it anyway would let it fall on their next
    // observation and hand back ground already counted.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let both = seed_user(&pool).await;
    let target_only: i64 = sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('bob', 'x', 0) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    for (user, uuid) in [(both, &target), (both, &source), (target_only, &target)] {
        sqlx::query(
            "INSERT INTO reading_progress_marks
                 (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
             VALUES (?, ?, 'epub', 50, 4000, 4000)",
        )
        .bind(user)
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(both))
        .await
        .unwrap();

    let clocks: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT user_id, sitting_observed_at FROM reading_progress_marks")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(clocks.len(), 2, "one surviving mark per reader");
    assert_eq!(
        clocks.iter().find(|(u, _)| *u == both).unwrap().1,
        None,
        "the reader who held both books re-baselines"
    );
    assert_eq!(
        clocks.iter().find(|(u, _)| *u == target_only).unwrap().1,
        Some(4000),
        "the reader who held only the target keeps their sitting"
    );
}
