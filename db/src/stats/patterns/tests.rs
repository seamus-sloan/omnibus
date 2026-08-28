//! Unit tests for [`super::time_patterns`]. The cases that matter are the
//! ones UTC bucketing gets wrong: a reader outside UTC, a sit crossing an
//! hour boundary, and an offset that carries a session onto a different local
//! day than the one its UTC timestamp names.

use super::*;
use crate::init_db;
use crate::test_support::{seed_minimal_books, seed_user};

/// 2024-01-01T00:00:00Z — a Monday, so weekday arithmetic reads off the
/// anchor without a lookup.
const JAN_1: i64 = 1_704_067_200;
const HOUR: i64 = 3_600;
const DAY: i64 = 86_400;

/// 2024-01-03T00:00:00Z, a **Wednesday** — the anchor most cases shift from.
const WED: i64 = JAN_1 + 2 * DAY;

/// Monday-first weekday indices, matching [`super::WEEKDAY_LABELS`].
const TUE: usize = 1;
const WED_IDX: usize = 2;

async fn pool_with_user() -> (SqlitePool, i64) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    (pool, user)
}

async fn reading_session(
    pool: &SqlitePool,
    user: i64,
    started_at: i64,
    secs: i64,
    offset_minutes: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO reading_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_read, utc_offset_minutes)
         VALUES (?, 'uuid-1', ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .bind(offset_minutes)
    .execute(pool)
    .await
    .unwrap();
}

async fn listening_session(
    pool: &SqlitePool,
    user: i64,
    started_at: i64,
    secs: i64,
    offset_minutes: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened, utc_offset_minutes)
         VALUES (?, 'uuid-1', ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .bind(offset_minutes)
    .execute(pool)
    .await
    .unwrap();
}

/// Seconds in one local hour, by index.
fn hour(p: &TimePatterns, h: usize) -> i64 {
    p.hour_of_day[h].seconds
}

/// Seconds on one local weekday, by Monday-first index.
fn weekday(p: &TimePatterns, d: usize) -> i64 {
    p.day_of_week[d].seconds
}

#[tokio::test]
async fn time_patterns_returns_every_hour_and_weekday_zero_filled_for_an_empty_window() {
    let (pool, user) = pool_with_user().await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    // AC5: the shapes are fixed-width, so an empty window is 24 and 7 zeros
    // rather than two empty vecs a renderer would have to guess the width of.
    assert_eq!(p.hour_of_day.len(), 24);
    assert_eq!(p.day_of_week.len(), 7);
    assert!(p.hour_of_day.iter().all(|b| b.seconds == 0));
    assert!(p.day_of_week.iter().all(|b| b.seconds == 0));
    assert_eq!(p.unzoned_seconds, 0);
    assert_eq!(p.hour_of_day[0].hour, 0);
    assert_eq!(p.hour_of_day[23].hour, 23);
    assert_eq!(p.day_of_week[0].label, "Mon");
    assert_eq!(p.day_of_week[6].label, "Sun");
}

#[tokio::test]
async fn time_patterns_buckets_a_non_utc_reader_into_their_own_local_hour() {
    let (pool, user) = pool_with_user().await;
    // Wednesday 04:00 UTC, recorded at UTC-7 — the reader's Tuesday evening.
    reading_session(&pool, user, WED + 4 * HOUR, 600, Some(-420)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    assert_eq!(hour(&p, 21), 600, "21:00 local is where the reading was");
    assert_eq!(hour(&p, 4), 0, "04:00 is the UTC answer, and it is wrong");
}

#[tokio::test]
async fn time_patterns_places_a_session_on_the_local_weekday_when_the_offset_crosses_midnight() {
    let (pool, user) = pool_with_user().await;
    // The same Wednesday-morning-UTC row: at UTC-7 it is still Tuesday.
    reading_session(&pool, user, WED + 4 * HOUR, 600, Some(-420)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    assert_eq!(weekday(&p, TUE), 600);
    assert_eq!(weekday(&p, WED_IDX), 0, "the UTC day is not the local day");
}

#[tokio::test]
async fn time_patterns_splits_a_sit_spanning_an_hour_boundary_across_both_hours() {
    let (pool, user) = pool_with_user().await;
    // The web tracker's 60s `rollover()` already delivers a long sit as
    // successive checkpoint rows, so the minutes either side of 21:00 arrive
    // pre-split and each lands in its own hour. A later change that coalesced
    // these into one row starting at 20:55 would pile all 900s onto hour 20.
    reading_session(&pool, user, WED + 20 * HOUR + 55 * 60, 300, Some(0)).await;
    reading_session(&pool, user, WED + 21 * HOUR, 600, Some(0)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    assert_eq!(hour(&p, 20), 300);
    assert_eq!(hour(&p, 21), 600);
    assert_eq!(weekday(&p, WED_IDX), 900, "both halves are the same day");
}

#[tokio::test]
async fn time_patterns_counts_listening_alongside_reading() {
    let (pool, user) = pool_with_user().await;
    reading_session(&pool, user, WED + 9 * HOUR, 300, Some(0)).await;
    listening_session(&pool, user, WED + 9 * HOUR, 700, Some(0)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    // AC6: both formats, like every other activity metric on the page.
    assert_eq!(hour(&p, 9), 1000);
    assert_eq!(weekday(&p, WED_IDX), 1000);
}

#[tokio::test]
async fn time_patterns_is_exact_for_a_half_hour_zone() {
    let (pool, user) = pool_with_user().await;
    // 00:45 UTC at UTC+05:30 is 06:15 local. Rotating 24 whole-hour buckets
    // by the offset — the shortcut this module deliberately avoids — would
    // report 05:00 instead.
    reading_session(&pool, user, WED + 45 * 60, 600, Some(330)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    assert_eq!(hour(&p, 6), 600);
    assert_eq!(hour(&p, 5), 0);
}

#[tokio::test]
async fn time_patterns_excludes_rows_with_no_offset_and_reports_their_seconds() {
    let (pool, user) = pool_with_user().await;
    reading_session(&pool, user, WED + 9 * HOUR, 300, Some(0)).await;
    reading_session(&pool, user, WED + 9 * HOUR, 700, None).await;
    listening_session(&pool, user, WED + 9 * HOUR, 50, None).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    // Excluded rather than folded into the UTC hour they happen to sit in:
    // the strips describe only the seconds that can be placed on a clock.
    assert_eq!(hour(&p, 9), 300);
    assert_eq!(p.unzoned_seconds, 750);
}

#[tokio::test]
async fn time_patterns_ignores_sessions_before_the_window_start() {
    let (pool, user) = pool_with_user().await;
    reading_session(&pool, user, WED - DAY + 9 * HOUR, 600, Some(0)).await;
    reading_session(&pool, user, WED + 9 * HOUR, 300, Some(0)).await;
    // Unzoned seconds are windowed the same way, or the disclosure would
    // describe a period the charts don't cover.
    reading_session(&pool, user, WED - DAY + 9 * HOUR, 900, None).await;

    let p = time_patterns(&pool, user, WED).await.unwrap();

    assert_eq!(hour(&p, 9), 300);
    assert_eq!(p.unzoned_seconds, 0);
}

#[tokio::test]
async fn time_patterns_keeps_one_users_sessions_out_of_anothers_buckets() {
    let (pool, user) = pool_with_user().await;
    let other = seed_user(&pool, "someone-else").await;
    reading_session(&pool, user, WED + 9 * HOUR, 300, Some(0)).await;
    reading_session(&pool, other, WED + 9 * HOUR, 9_000, Some(0)).await;

    let p = time_patterns(&pool, user, 0).await.unwrap();

    assert_eq!(hour(&p, 9), 300);
}

#[tokio::test]
async fn time_patterns_propagates_a_db_error_when_the_session_table_is_gone() {
    let (pool, user) = pool_with_user().await;
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = time_patterns(&pool, user, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)), "got: {err:?}");
}
