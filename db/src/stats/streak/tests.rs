//! Unit tests for [`super::streak`] and its two pure run helpers. The run
//! math is exercised directly on day lists — cheap and exhaustive — with the
//! DB tests covering the query that produces those lists.

use super::*;
use crate::init_db;
use crate::test_support::{seed_minimal_books, seed_user};

const DAY: i64 = 86_400;

/// A late-2023 anchor, and the day number it falls on.
const T0: i64 = 1_700_000_000;
const T0_DAY: i64 = T0 / DAY;

async fn reading_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
         VALUES (?, ?, ?, ?, 600)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + 600)
    .execute(pool)
    .await
    .unwrap();
}

async fn listening_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened)
         VALUES (?, ?, ?, ?, 600)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + 600)
    .execute(pool)
    .await
    .unwrap();
}

// --- longest_run --------------------------------------------------------

#[test]
fn longest_run_is_zero_for_an_empty_day_list() {
    assert_eq!(longest_run(&[]), 0);
}

#[test]
fn longest_run_takes_the_best_run_across_a_gap_day() {
    // Days 0,1,2 (run 3), gap, then 5,6 (run 2).
    assert_eq!(longest_run(&[0, 1, 2, 5, 6]), 3);
}

#[test]
fn longest_run_is_one_for_a_single_day() {
    assert_eq!(longest_run(&[9]), 1);
}

// --- current_run --------------------------------------------------------

#[test]
fn current_run_is_zero_when_there_are_no_active_days() {
    assert_eq!(current_run(&[], 100), 0);
}

#[test]
fn current_run_counts_a_run_ending_today() {
    assert_eq!(current_run(&[98, 99, 100], 100), 3);
}

#[test]
fn current_run_stays_live_when_today_is_inactive_but_yesterday_was() {
    // Not read *yet* today — the streak survives until the day is over.
    assert_eq!(current_run(&[97, 98, 99], 100), 3);
}

#[test]
fn current_run_is_zero_once_the_run_ends_before_yesterday() {
    // The day before yesterday: the streak is over, not merely idle.
    assert_eq!(current_run(&[96, 97, 98], 100), 0);
}

#[test]
fn current_run_is_one_for_a_single_active_day() {
    assert_eq!(current_run(&[100], 100), 1);
    assert_eq!(current_run(&[99], 100), 1);
}

#[test]
fn current_run_stops_at_the_gap_and_ignores_the_earlier_streak() {
    // A long-ago run of 4 must not be added to the live run of 2.
    assert_eq!(current_run(&[80, 81, 82, 83, 99, 100], 100), 2);
}

#[test]
fn current_run_ignores_a_session_dated_in_the_future() {
    // A device with a fast clock files a session 30 days out. Counted as the
    // head of the run, the gap behind it breaks the walk and a 3-day streak
    // reports 1.
    assert_eq!(current_run(&[98, 99, 100, 130], 100), 3);
    // And a future day must not stand in for activity the reader never had.
    assert_eq!(current_run(&[130], 100), 0);
    assert_eq!(current_run(&[60, 130], 100), 0);
}

// --- streak -------------------------------------------------------------

#[tokio::test]
async fn streak_reports_active_days_longest_and_current_from_one_day_list() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Days 0,1,2 (longest 3), gap, then 5,6 — with "today" = day 6, so the
    // trailing pair is the live run.
    for d in [0, 1, 2, 5, 6] {
        reading_session(&pool, user, "uuid-1", T0 + d * DAY).await;
    }

    let s = streak(&pool, user, 0, T0_DAY + 6).await.unwrap();

    assert_eq!(s.active_days, 5);
    assert_eq!(s.longest_days, 3);
    assert_eq!(s.current_days, 2);
}

#[tokio::test]
async fn streak_counts_a_listening_only_day_towards_the_current_run() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Read yesterday, listened today: both are activity, so this is a 2-day
    // run — the same union every other activity metric on the page uses.
    reading_session(&pool, user, "uuid-1", T0).await;
    listening_session(&pool, user, "uuid-1", T0 + DAY).await;

    let s = streak(&pool, user, 0, T0_DAY + 1).await.unwrap();

    assert_eq!(s.active_days, 2);
    assert_eq!(s.current_days, 2);
}

#[tokio::test]
async fn streak_is_all_zero_for_a_user_with_no_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let s = streak(&pool, user, 0, T0_DAY).await.unwrap();

    assert_eq!(s.active_days, 0);
    assert_eq!(s.longest_days, 0);
    assert_eq!(s.current_days, 0, "no sessions is a zero streak, not one");
}

#[tokio::test]
async fn streak_reports_the_live_run_from_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Ten days running, but the window opens two days ago — the shape of a
    // reader opening the Month tab on the 2nd. Days active and the record are
    // the window's; the streak is the reader's.
    for back in 0..10 {
        reading_session(&pool, user, "uuid-1", T0 - back * DAY).await;
    }
    let window_start = T0 - DAY;

    let s = streak(&pool, user, window_start, T0_DAY).await.unwrap();

    assert_eq!(s.active_days, 2, "days active stay scoped to the window");
    assert_eq!(s.longest_days, 2, "so does the record");
    assert_eq!(
        s.current_days, 10,
        "the live run is a fact about now, not about the reporting period"
    );
}

#[tokio::test]
async fn streak_reports_current_zero_when_the_last_session_is_stale() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0).await;

    let s = streak(&pool, user, 0, T0_DAY + 30).await.unwrap();

    assert_eq!(s.longest_days, 1, "the record survives");
    assert_eq!(s.current_days, 0, "the run does not");
}

#[tokio::test]
async fn streak_propagates_sqlx_error_when_the_sessions_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = streak(&pool, 1, 0, T0_DAY).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
