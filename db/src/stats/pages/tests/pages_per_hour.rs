//! `pages_per_hour`: resolved pages over recorded reading hours, weighted
//! by seconds, reading time from before the window counted, listening and
//! unmeasured or zero-page books excluded, a book finished both ways
//! counted once, the `None` cases, and `hourly_rate`.

use super::super::*;
use super::{finish_journal, finish_read_status, listen_session, seed_book, seed_lib, T0};
use crate::init_db;
use crate::test_support::seed_user;

async fn read_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
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

#[tokio::test]
async fn pages_per_hour_divides_resolved_pages_by_recorded_reading_hours() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // 300 pages over 10 hours.
    read_session(&pool, user, "uuid-a", T0 - 3600, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_weights_by_seconds_rather_than_averaging_per_book_rates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // 100 pages in 1h (100/h) and 400 pages in 9h (~44/h). A mean of the two
    // per-book rates would be ~72; the seconds-weighted answer is 50, and the
    // long book is the one that describes how this reader actually reads.
    seed_book(&pool, lib, "uuid-fast", None, Some(100)).await;
    seed_book(&pool, lib, "uuid-slow", None, Some(400)).await;
    finish_journal(&pool, user, "uuid-fast", T0).await;
    finish_journal(&pool, user, "uuid-slow", T0).await;
    read_session(&pool, user, "uuid-fast", T0, 3600).await;
    read_session(&pool, user, "uuid-slow", T0, 9 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 50.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_counts_reading_time_from_before_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // Nine of the ten hours were read long before the window opened. Counting
    // only the in-window hour would report 300 pages/hour for a book that
    // actually took ten.
    read_session(&pool, user, "uuid-a", T0 - 90 * 86_400, 9 * 3600).await;
    read_session(&pool, user, "uuid-a", T0, 3600).await;

    let rate = pages_per_hour(&pool, user, T0 - 3600)
        .await
        .unwrap()
        .unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_excludes_a_finished_book_with_no_recorded_reading_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-timed", None, Some(300)).await;
    // Finished on another device, or before session tracking: its pages with
    // nobody's hours behind them would double the rate.
    seed_book(&pool, lib, "uuid-untimed", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-timed", T0).await;
    finish_journal(&pool, user, "uuid-untimed", T0).await;
    read_session(&pool, user, "uuid-timed", T0, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_excludes_listening_time_from_the_denominator() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    // Narration speed is the narrator's; folding it in would drag the rate to
    // 15/h and stop measuring reading at all.
    listen_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_counts_a_book_finished_both_ways_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    // Both sides double under a non-DISTINCT scope, so a wrong join here
    // hides in the ratio — the count each side is over is what this pins.
    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_is_none_when_no_finished_book_has_both_a_length_and_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Length but no time.
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    // Time but no resolvable length (audio-only / not-yet-backfilled).
    seed_book(&pool, lib, "uuid-b", None, None).await;
    finish_journal(&pool, user, "uuid-b", T0).await;
    read_session(&pool, user, "uuid-b", T0, 3600).await;

    assert_eq!(pages_per_hour(&pool, user, 0).await.unwrap(), None);
}

/// A `word_count` of 0 is a real stored value — `estimate_word_count` returns
/// `Some(0)` for an EPUB whose spine loads but strips to no words — and the
/// ladder turns it into 0 pages, not NULL. Summing that costs `pages_read`
/// nothing, but here it would donate hours against no pages.
#[tokio::test]
async fn pages_per_hour_excludes_a_zero_page_book_from_both_sides() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    // Image-only EPUB: spine loaded, no extractable text.
    seed_book(&pool, lib, "uuid-zero", Some(0), None).await;
    finish_journal(&pool, user, "uuid-zero", T0).await;
    read_session(&pool, user, "uuid-zero", T0, 6 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    // 30/h from the measurable book alone. Counting the zero-page book's
    // six hours in the denominator would give 300/16h = 18.75.
    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_is_none_when_the_only_finished_book_measures_zero_pages() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-zero", Some(0), None).await;
    finish_journal(&pool, user, "uuid-zero", T0).await;
    read_session(&pool, user, "uuid-zero", T0, 6 * 3600).await;

    // Not `Some(0.0)` — "0 pages an hour" is a claim about how this reader
    // reads, and an unmeasurable book is not that claim.
    assert_eq!(pages_per_hour(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_per_hour_is_none_when_nothing_finished_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;

    assert_eq!(pages_per_hour(&pool, user, T0 + 1).await.unwrap(), None);
}

#[tokio::test]
async fn pages_per_hour_ignores_another_users_reading_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let other = seed_user(&pool, "bob").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(300)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;
    read_session(&pool, user, "uuid-a", T0, 10 * 3600).await;
    read_session(&pool, other, "uuid-a", T0, 90 * 3600).await;

    let rate = pages_per_hour(&pool, user, 0).await.unwrap().unwrap();

    assert!((rate - 30.0).abs() < 1e-9, "{rate}");
}

#[tokio::test]
async fn pages_per_hour_propagates_sqlx_error_when_the_books_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_per_hour(&pool, 1, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}

#[test]
fn hourly_rate_is_none_without_both_sides_or_with_no_time() {
    assert_eq!(hourly_rate(None, Some(3600)), None);
    assert_eq!(hourly_rate(Some(300), None), None);
    assert_eq!(hourly_rate(Some(300), Some(0)), None);
    assert_eq!(hourly_rate(Some(300), Some(3600)), Some(300.0));
}
