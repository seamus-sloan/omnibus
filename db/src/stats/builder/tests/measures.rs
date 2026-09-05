//! One end-to-end `chart_series` pass per measure and grain: completions
//! (counted once when doubly recorded, dropped with their book), average
//! length, reading minutes and sittings from the session tables, ratings,
//! ledger pages, per-user scoping, the empty window, week buckets, and
//! activity dated after today.

use omnibus_shared::{ChartMark, StatsRange};

use super::super::*;
use super::{at, fixture, ledger_day, set_pages, spec};
use crate::stats::tests::{
    book_id, finish_journal, listening_session, months_ago_secs, rate_book, reading_session,
    seed_user,
};

/// Mark a book finished through the read-status path (the other half of the
/// `FINISHED_EVENTS` union from `finish_journal`).
async fn finish_status(pool: &sqlx::SqlitePool, user: i64, uuid: &str, at: i64) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(at)
    .bind(at)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn chart_series_counts_books_finished_per_month() {
    let (pool, user) = fixture(3).await;
    let this_month = months_ago_secs(&pool, 0).await;
    let last_month = months_ago_secs(&pool, 1).await;
    finish_journal(&pool, user, "uuid-1", last_month).await;
    finish_journal(&pool, user, "uuid-2", this_month).await;
    finish_journal(&pool, user, "uuid-3", this_month).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    let last = r.buckets.len() - 1;
    assert_eq!(at(&r, 0, last), Some(2.0));
    assert_eq!(at(&r, 0, last - 1), Some(1.0));
    assert_eq!(r.series[0].mark, ChartMark::Bar);
    assert_eq!(r.axes[0].unit, ChartUnit::Books);
}

#[tokio::test]
async fn chart_series_counts_a_doubly_recorded_completion_once() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    // The same completion reaching both halves of the FINISHED_EVENTS union.
    finish_journal(&pool, user, "uuid-1", now).await;
    finish_status(&pool, user, "uuid-1", now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(1.0));
}

#[tokio::test]
async fn chart_series_averages_the_length_of_the_books_finished_in_each_bucket() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    set_pages(&pool, "uuid-1", 200).await;
    set_pages(&pool, "uuid-2", 400).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    finish_journal(&pool, user, "uuid-2", now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::AvgPageLength],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(300.0));
    assert_eq!(r.series[0].mark, ChartMark::Line);
}

#[tokio::test]
async fn chart_series_sums_reading_minutes_from_the_sitting_grain() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 1_800).await;
    reading_session(&pool, user, "uuid-1", now + 60, 600).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::ReadingMinutes],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(40.0));
}

#[tokio::test]
async fn chart_series_counts_sittings_across_both_session_tables() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 600).await;
    listening_session(&pool, user, "uuid-1", now + 60, 1_200).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::SessionCount],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(2.0));
}

#[tokio::test]
async fn chart_series_averages_ratings_on_the_rating_grain() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    rate_book(&pool, user, "uuid-1", 8, now).await;
    rate_book(&pool, user, "uuid-2", 10, now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::AvgRating],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    // (4 + 5) / 2 stars.
    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(4.5));
    assert_eq!(r.axes[0].unit, ChartUnit::Stars);
}

#[tokio::test]
async fn chart_series_reads_pages_from_the_progress_ledger() {
    let (pool, user) = fixture(1).await;
    set_pages(&pool, "uuid-1", 300).await;
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&pool)
        .await
        .unwrap();
    ledger_day(&pool, user, "uuid-1", &today, 20).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::PagesRead],
            ChartBucket::Day,
            StatsRange::Month,
        ),
        None,
    )
    .await
    .unwrap();

    // 20% of 300 pages.
    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(60.0));
}

#[tokio::test]
async fn chart_series_drops_a_zero_page_book_from_the_length_average() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    set_pages(&pool, "uuid-1", 300).await;
    set_pages(&pool, "uuid-2", 0).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    finish_journal(&pool, user, "uuid-2", now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::AvgPageLength],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    // 300, not 150 — an unmeasurable book donates nothing to a mean.
    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(300.0));
}

#[tokio::test]
async fn chart_series_scopes_every_measure_to_the_asking_user() {
    let (pool, user) = fixture(2).await;
    let other = seed_user(&pool, "someone-else").await;
    let now = months_ago_secs(&pool, 0).await;
    finish_journal(&pool, other, "uuid-1", now).await;
    reading_session(&pool, other, "uuid-2", now, 3_600).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished, ChartMeasure::ReadingMinutes],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert!(r.is_empty());
}

#[tokio::test]
async fn chart_series_returns_an_empty_result_for_a_window_with_nothing_in_it() {
    let (pool, user) = fixture(1).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert!(r.buckets.is_empty());
    assert!(r.series.is_empty());
    assert!(r.is_empty());
    assert!(!r.truncated);
}

#[tokio::test]
async fn chart_series_buckets_a_week_to_its_monday() {
    let (pool, user) = fixture(1).await;
    // A Wednesday, so the week key must come back as the Monday before it.
    let wednesday: i64 = sqlx::query_scalar(
        "SELECT CAST(strftime('%s', date('now', 'weekday 0', '-4 days')) AS INTEGER)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    finish_journal(&pool, user, "uuid-1", wednesday).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Week,
            StatsRange::AllTime,
        ),
        None,
    )
    .await
    .unwrap();

    let monday: String = sqlx::query_scalar("SELECT date('now', 'weekday 0', '-6 days')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(r.buckets[0], monday);
    assert_eq!(r.series[0].values[0], Some(1.0));
}

#[tokio::test]
async fn chart_series_ignores_a_completion_on_a_book_that_no_longer_exists() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    // A ghosted book: the completion survives, the book row does not.
    let id = book_id(&pool, "uuid-1").await;
    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap();

    assert!(r.is_empty());
}

#[tokio::test]
async fn chart_series_drops_activity_dated_after_today() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 600).await;
    // Nothing bounds `SessionReport.started_at` above, so a device with a fast
    // clock can file one — `streak` guards against the same thing.
    let ahead: i64 = sqlx::query_scalar("SELECT CAST(strftime('%s','now','+5 months') AS INTEGER)")
        .fetch_one(&pool)
        .await
        .unwrap();
    reading_session(&pool, user, "uuid-1", ahead, 6_000).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::ReadingMinutes],
            ChartBucket::Month,
            StatsRange::AllTime,
        ),
        None,
    )
    .await
    .unwrap();

    // The axis stops at today rather than stretching five empty months to
    // reach it, so the future sitting is left out — and only the real one is
    // plotted. The `/stats` totals still count it, which is the cost.
    //
    // Derived from the same clock the fixture and the query use: a pinned
    // month literal here passes only until the calendar leaves it behind.
    let this_month: String = sqlx::query_scalar("SELECT strftime('%Y-%m', 'now')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(!r.buckets.iter().any(|b| b.as_str() > this_month.as_str()));
    let charted: f64 = r.series[0].values.iter().flatten().sum();
    assert_eq!(charted, 10.0);
}
