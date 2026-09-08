//! Where a window opens and what the previous period compares against:
//! the rolling week, the calendar month and year, the zeroed all-time
//! baseline, and the previous-period bounds covering only the elapsed
//! slice of the prior period.

use omnibus_shared::{PeriodComparison, StatsRange};

use super::super::*;
use super::{
    drop_book_row, finish_journal, finish_read_status, listening_session, months_ago_secs,
    prev_period_start, rate_book, reading_session, seed_user, DAY, T0,
};
use crate::init_db;
use crate::test_support::seed_minimal_books;

#[tokio::test]
async fn week_window_keeps_only_the_rolling_last_seven_days() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // 8 days back is outside the rolling window even at start-of-day
    // granularity; a just-now session is inside it.
    let now = now_secs();
    reading_session(&pool, user, "uuid-1", now - 8 * DAY, 600).await;
    reading_session(&pool, user, "uuid-1", now, 300).await;

    let s = compute(&pool, user, StatsRange::Week, 0).await.unwrap();
    assert_eq!(s.reading_seconds, 300);
    assert_eq!(s.sessions, 1);
}

#[tokio::test]
async fn month_window_starts_at_the_first_of_the_current_month() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // 32 days back always lands in a previous month; a just-now session is in
    // the current one.
    let now = now_secs();
    reading_session(&pool, user, "uuid-1", now - 32 * DAY, 600).await;
    reading_session(&pool, user, "uuid-1", now, 300).await;

    let s = compute(&pool, user, StatsRange::Month, 0).await.unwrap();
    assert_eq!(s.reading_seconds, 300);
    assert_eq!(s.sessions, 1);
}

#[tokio::test]
async fn year_window_excludes_old_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // T0 is in 2023; the Year window (current calendar year) excludes it.
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let s = compute(&pool, user, StatsRange::Year, 0).await.unwrap();
    assert_eq!(s.reading_seconds, 0);
    assert!(s.is_empty());
}

#[tokio::test]
async fn previous_period_is_zeroed_for_all_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    listening_session(&pool, user, "uuid-1", T0, 600).await;

    let prev = previous_period(&pool, user, StatsRange::AllTime, 0)
        .await
        .unwrap();
    assert_eq!(prev, PeriodComparison::default());
}

#[tokio::test]
async fn previous_period_month_sums_only_last_calendar_months_activity() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Anchored to the first second of last month rather than its middle: the
    // baseline is the *elapsed* slice of the previous period, so a mid-month
    // seed would fall outside it whenever the suite runs early in a month.
    let last_month = prev_period_start(&pool, StatsRange::Month).await;
    let two_months_back = months_ago_secs(&pool, 2).await;
    let now = now_secs();

    listening_session(&pool, user, "uuid-1", last_month, 500).await;
    // Outside the previous window — must not be counted.
    listening_session(&pool, user, "uuid-1", two_months_back, 999).await;
    listening_session(&pool, user, "uuid-1", now, 111).await;
    rate_book(&pool, user, "uuid-1", 10, last_month).await;
    finish_journal(&pool, user, "uuid-1", last_month).await;

    let prev = previous_period(&pool, user, StatsRange::Month, 0)
        .await
        .unwrap();
    assert_eq!(prev.listening_seconds, 500);
    assert_eq!(prev.avg_stars, Some(5.0));
    assert_eq!(prev.books_finished, 1);
}

#[tokio::test]
async fn previous_period_excludes_a_completion_whose_book_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    // The first second of last month. Not mid-month: the baseline is now the
    // *elapsed* slice of the previous period, so a day-15 seed falls outside it
    // for the first fortnight of every month.
    let prev = prev_period_start(&pool, StatsRange::Month).await;
    finish_read_status(&pool, user, "uuid-1", prev).await;
    finish_read_status(&pool, user, "uuid-2", prev).await;
    drop_book_row(&pool, "uuid-2").await;

    // The delta's baseline must count the same population the current window
    // does, or the drill-in invents a drop the reader never had.
    let previous = previous_period(&pool, user, StatsRange::Month, 0)
        .await
        .unwrap();
    assert_eq!(previous.books_finished, 1);
}

#[tokio::test]
async fn prev_window_bounds_cover_the_elapsed_slice_of_the_previous_period() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    for range in [StatsRange::Week, StatsRange::Month, StatsRange::Year] {
        let (start, end) = prev_window_bounds(&pool, range, 0).await.unwrap().unwrap();
        let cur_start = window_start(&pool, range, 0).await.unwrap();
        let elapsed = now_secs() - cur_start;

        // The baseline sits wholly before the current window …
        assert!(
            end <= cur_start,
            "{range:?}: baseline must not overlap the current window ({end} > {cur_start})"
        );
        // Empty only at the exact first second of a period, where the current
        // window is equally empty — asserted as a bound, not as non-emptiness.
        assert!(start <= end, "{range:?}: baseline must not be inverted");
        // … and covers the same elapsed offset, never the whole period. The
        // slack absorbs the second or two between the two `now` reads; the
        // clamp can only ever make the slice shorter, never longer.
        let slice = end - start;
        assert!(
            slice <= elapsed + 2,
            "{range:?}: baseline slice {slice}s exceeds the elapsed {elapsed}s"
        );
    }
}

#[tokio::test]
async fn previous_period_aggregates_only_within_the_baseline_bounds() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    // Seeded from the bounds themselves, so this asserts that the aggregates
    // honour the window — that the window is the *right* window is
    // `prev_window_bounds_cover_the_elapsed_slice_of_the_previous_period`'s
    // job, and only that test detects a regression in the bounds arithmetic.
    let (start, end) = prev_window_bounds(&pool, StatsRange::Week, 0)
        .await
        .unwrap()
        .unwrap();
    listening_session(&pool, user, "uuid-1", start, 500).await;
    // `end` is exclusive. Seeded a second past it: `previous_period` re-reads
    // the bounds, and for Week `end` advances with the wall clock, so a seed
    // exactly on it can slip inside when a second boundary falls between.
    listening_session(&pool, user, "uuid-1", end + 1, 999).await;
    listening_session(&pool, user, "uuid-1", start - 1, 777).await;

    let prev = previous_period(&pool, user, StatsRange::Week, 0)
        .await
        .unwrap();
    assert_eq!(prev.listening_seconds, 500);
}

#[test]
fn prev_window_from_takes_the_elapsed_offset_into_the_previous_period() {
    // Fixed dates, so the clamp and the degenerate cases are exercised on every
    // run rather than on the handful of calendar days that reach them live.
    const DAY: i64 = 86_400;
    // 2026-03-01 00:00 UTC, 2026-02-01 00:00 UTC.
    let mar1 = 1_772_323_200;
    let feb1 = mar1 - 28 * DAY;

    // Three days into March → the first three days of February.
    let (s, e) = prev_window_from(feb1, mar1, mar1 + 3 * DAY);
    assert_eq!((s, e), (feb1, feb1 + 3 * DAY));

    // Thirty days into March → clamped to the whole of a 28-day February,
    // never past the period it belongs to.
    let (s, e) = prev_window_from(feb1, mar1, mar1 + 30 * DAY);
    assert_eq!((s, e), (feb1, mar1));
    assert_eq!(e - s, 28 * DAY);

    // The exact first second of the period: nothing elapsed, so the baseline
    // is empty rather than the whole previous month.
    assert_eq!(prev_window_from(feb1, mar1, mar1), (feb1, feb1));

    // A clock that reads behind the period start cannot invert the window.
    assert_eq!(prev_window_from(feb1, mar1, mar1 - 5), (feb1, feb1));
}
