//! Unit tests for [`super::book_insights`]: the happy-path aggregate, the
//! `None` (em-dash) paths — no sessions, unresolvable uuid, glances only —
//! user isolation, the merged-uuid resolve, the propagated `StatsError::Sqlx`
//! variant, and the sitting stitch (cadence, idle gap, cross-format, floor).

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn reading_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

async fn listening_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions (user_id, book_uuid, started_at, ended_at, seconds_listened)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

const T0: i64 = 1_700_000_000;

#[tokio::test]
async fn book_insights_returns_none_when_book_has_no_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    assert_eq!(book_insights(&pool, user, "uuid-1").await.unwrap(), None);
}

#[tokio::test]
async fn book_insights_returns_none_when_uuid_does_not_resolve_to_a_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert_eq!(
        book_insights(&pool, user, "no-such-uuid").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn book_insights_aggregates_started_time_and_session_count_across_formats() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    listening_session(&pool, user, "uuid-1", T0 - 3600, 300).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.started_at, T0 - 3600);
    assert_eq!(insights.seconds_total, 900);
    assert_eq!(insights.sessions, 2);
}

#[tokio::test]
async fn book_insights_only_counts_sessions_for_the_given_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    reading_session(&pool, alice, "uuid-1", T0, 600).await;
    reading_session(&pool, bob, "uuid-1", T0, 1200).await;

    let alice_insights = book_insights(&pool, alice, "uuid-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice_insights.seconds_total, 600);
    assert_eq!(alice_insights.sessions, 1);

    assert_eq!(
        book_insights(&pool, bob, "uuid-1")
            .await
            .unwrap()
            .unwrap()
            .sessions,
        1
    );
}

#[tokio::test]
async fn book_insights_resolves_through_merged_uuids_to_the_canonical_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('absorbed-uuid', ?, 'EPUB', '/lib/absorbed')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    // Requested via the absorbed (merged-away) uuid; sessions were recorded
    // under the surviving book's canonical uuid.
    let insights = book_insights(&pool, user, "absorbed-uuid")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(insights.sessions, 1);
    assert_eq!(insights.seconds_total, 600);
}

#[tokio::test]
async fn book_insights_propagates_sqlx_error_when_sessions_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = book_insights(&pool, user, "uuid-1").await.unwrap_err();
    assert!(matches!(err, StatsError::Sqlx(_)), "got: {err:?}");
}

#[tokio::test]
async fn book_insights_reports_longest_sit_breaking_ties_to_the_earliest() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    listening_session(&pool, user, "uuid-1", T0 + 7_200, 1_800).await;
    // Same length as the winner, but later — the earlier one must win.
    reading_session(&pool, user, "uuid-1", T0 + 86_400, 1_800).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.longest_seconds, 1_800);
    assert_eq!(insights.longest_started_at, T0 + 7_200);
}

#[tokio::test]
async fn book_insights_buckets_daily_activity_by_utc_day_ascending() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // T0 = 2023-11-14 22:13:20 UTC. Two sessions the same UTC day (reading +
    // listening merge into one bucket), one two days later.
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    listening_session(&pool, user, "uuid-1", T0 + 60, 300).await;
    reading_session(&pool, user, "uuid-1", T0 + 2 * 86_400, 240).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(
        insights.daily,
        vec![
            DayActivity {
                day: "2023-11-14".into(),
                seconds: 900
            },
            DayActivity {
                day: "2023-11-16".into(),
                seconds: 240
            },
        ]
    );
    // Stamped from the server clock — only its shape is stable in a test.
    assert_eq!(insights.as_of_day.len(), 10);
}

/// An hour of web reading: sixty contiguous 60s heartbeat flushes, the shape
/// `session_tracker`'s rollover writes.
async fn heartbeat_hour(pool: &SqlitePool, user: i64, uuid: &str, from: i64) {
    for i in 0..60 {
        reading_session(pool, user, uuid, from + i * 60, 60).await;
    }
}

#[tokio::test]
async fn book_insights_stitches_contiguous_checkpoint_rows_into_one_sitting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    heartbeat_hour(&pool, user, "uuid-1", T0).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 1, "an hour is one sitting, not 60 rows");
    assert_eq!(
        insights.seconds_total, 3_600,
        "the stitch must not lose time"
    );
    assert_eq!(insights.longest_seconds, 3_600);
    assert_eq!(insights.longest_started_at, T0);
}

#[tokio::test]
async fn book_insights_counts_the_same_hour_alike_whatever_the_flush_cadence() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    // Web: 60 × 60s. iOS: 12 × 300s. Same hour, same answer.
    heartbeat_hour(&pool, user, "uuid-1", T0).await;
    for i in 0..12 {
        reading_session(&pool, user, "uuid-2", T0 + i * 300, 300).await;
    }

    let web = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();
    let ios = book_insights(&pool, user, "uuid-2").await.unwrap().unwrap();

    assert_eq!(web.sessions, ios.sessions);
    assert_eq!(web.seconds_total, ios.seconds_total);
    assert_eq!(web.longest_seconds, ios.longest_seconds);
}

#[tokio::test]
async fn book_insights_splits_sittings_when_idle_longer_than_the_gap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    // Resumes 901s after the first ended — one second past the threshold.
    reading_session(&pool, user, "uuid-1", T0 + 600 + 901, 600).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 2);
    assert_eq!(insights.seconds_total, 1_200);
}

#[tokio::test]
async fn book_insights_keeps_one_sitting_when_idle_exactly_the_gap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    // Exactly the threshold is still the same sitting — the break is `>`.
    reading_session(&pool, user, "uuid-1", T0 + 600 + 900, 600).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 1);
    assert_eq!(insights.longest_seconds, 1_200);
}

#[tokio::test]
async fn book_insights_stitches_interleaved_reading_and_listening_into_one_sitting() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // A dual-format book switched between mid-stretch: the rows land in two
    // tables and overlap, so the stitch must run over the union.
    reading_session(&pool, user, "uuid-1", T0, 900).await;
    listening_session(&pool, user, "uuid-1", T0 + 300, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + 900, 600).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 1, "one stretch is one sitting per book");
    assert_eq!(insights.seconds_total, 2_100);
}

#[tokio::test]
async fn book_insights_reports_the_longest_stitched_sitting_not_the_longest_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Ten contiguous 60s flushes — a ten-minute sitting made of short rows.
    for i in 0..10 {
        reading_session(&pool, user, "uuid-1", T0 + i * 60, 60).await;
    }
    // The longest single *row*, but a shorter sitting.
    reading_session(&pool, user, "uuid-1", T0 + 86_400, 300).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.longest_seconds, 600);
    assert_eq!(insights.longest_started_at, T0);
}

#[tokio::test]
async fn book_insights_returns_none_when_every_sitting_is_under_the_minimum() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Three glances, days apart so they can't stitch into a real sitting.
    reading_session(&pool, user, "uuid-1", T0, 20).await;
    reading_session(&pool, user, "uuid-1", T0 + 86_400, 20).await;
    reading_session(&pool, user, "uuid-1", T0 + 2 * 86_400, 20).await;

    assert_eq!(book_insights(&pool, user, "uuid-1").await.unwrap(), None);
}

#[tokio::test]
async fn book_insights_keeps_glance_seconds_in_the_total_it_excludes_from_the_count() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + 86_400, 20).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 1, "the glance is not a sitting");
    assert_eq!(insights.seconds_total, 620, "but its seconds still count");
}

#[tokio::test]
async fn book_insights_counts_a_sitting_glances_add_up_to_over_the_minimum() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Four 20s dips within one stretch — individually glances, together a
    // sitting. The floor applies to the stitched total, not to a row.
    for i in 0..4 {
        reading_session(&pool, user, "uuid-1", T0 + i * 100, 20).await;
    }

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.sessions, 1);
    assert_eq!(insights.seconds_total, 80);
}
