//! Unit tests for the chart builder: the pure bucketing/alignment helpers,
//! one end-to-end pass per grain, and the cases the design exists to get
//! right — a dense axis, mixed grains on one bucket key, an average that
//! refuses to read as zero, and a genre fold that re-averages from sums.

use omnibus_shared::{ChartBreakdown, ChartMark, ChartSpec, StatsRange};

use super::*;
use crate::init_db;
use crate::stats::tests::{
    book_id, finish_journal, listening_session, months_ago_secs, rate_book, reading_session,
    seed_user, set_genres, DAY,
};
use crate::test_support::seed_minimal_books;

/// A pool with `count` books and one user, ready for session/completion seeds.
async fn fixture(count: i64) -> (sqlx::SqlitePool, i64) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, count).await;
    let user = seed_user(&pool, "reader").await;
    (pool, user)
}

/// Give a book a real `page_count` so the length ladder's first rung resolves.
async fn set_pages(pool: &sqlx::SqlitePool, uuid: &str, pages: i64) {
    sqlx::query("UPDATE books SET page_count = ? WHERE uuid = ?")
        .bind(pages)
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

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

/// A forward-progress ledger row on a given UTC day.
async fn ledger_day(pool: &sqlx::SqlitePool, user: i64, uuid: &str, day: &str, percent: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(percent)
    .execute(pool)
    .await
    .unwrap();
}

fn spec(measures: Vec<ChartMeasure>, bucket: ChartBucket, range: StatsRange) -> ChartSpec {
    ChartSpec {
        measures,
        bucket,
        range,
        breakdown: ChartBreakdown::None,
    }
}

/// The value a named series carries in the bucket at `idx`.
fn at(result: &ChartResult, series: usize, idx: usize) -> Option<f64> {
    result.series[series].values[idx]
}

// ── Pure helpers ────────────────────────────────────────────────────────

#[test]
fn bucket_expr_folds_a_day_to_each_granularity() {
    assert_eq!(bucket_expr(ChartBucket::Day, "day"), "day");
    assert_eq!(
        bucket_expr(ChartBucket::Week, "day"),
        "date(day, 'weekday 0', '-6 days')"
    );
    assert_eq!(bucket_expr(ChartBucket::Month, "day"), "substr(day, 1, 7)");
    assert_eq!(bucket_expr(ChartBucket::Year, "day"), "substr(day, 1, 4)");
}

#[test]
fn bucket_start_day_inverts_every_bucket_key() {
    assert_eq!(
        bucket_start_day(ChartBucket::Day, "2026-03-14"),
        "2026-03-14"
    );
    assert_eq!(
        bucket_start_day(ChartBucket::Week, "2026-03-09"),
        "2026-03-09"
    );
    assert_eq!(
        bucket_start_day(ChartBucket::Month, "2026-03"),
        "2026-03-01"
    );
    assert_eq!(bucket_start_day(ChartBucket::Year, "2026"), "2026-01-01");
}

#[tokio::test]
async fn a_count_axis_comes_back_with_whole_number_gridlines() {
    let (pool, user) = fixture(4).await;
    let now = months_ago_secs(&pool, 0).await;
    for i in 1..=2 {
        finish_journal(&pool, user, &format!("uuid-{i}"), now).await;
    }

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();

    let axis = &r.axes[0];
    let step = axis.max / f64::from(r.divisions);
    assert_eq!(step.fract(), 0.0, "gridline step {step} is not whole");
    assert_eq!(axis.max, 2.0);
}

#[test]
fn reduce_sums_totals_but_re_averages_from_the_row_counts() {
    let a = Bucketed {
        bucket: "2026-01".into(),
        total: 600.0,
        n: 2.0,
    };
    let b = Bucketed {
        bucket: "2026-01".into(),
        total: 300.0,
        n: 1.0,
    };
    let rows = vec![&a, &b];
    assert_eq!(reduce(&rows, ChartAggregate::Sum), Some(900.0));
    assert_eq!(reduce(&rows, ChartAggregate::Count), Some(900.0));
    // 900/3, not the mean of 300 and 300 — a two-book slice must not weigh
    // the same as a one-book slice.
    assert_eq!(reduce(&rows, ChartAggregate::Average), Some(300.0));
    assert_eq!(reduce(&[], ChartAggregate::Average), None);
}

#[test]
fn align_fills_an_absent_bucket_with_zero_for_totals_and_nothing_for_averages() {
    let buckets = vec!["2026-01".to_string(), "2026-02".into(), "2026-03".into()];
    let rows = vec![Bucketed {
        bucket: "2026-02".into(),
        total: 4.0,
        n: 2.0,
    }];
    assert_eq!(
        align(&buckets, &rows, ChartAggregate::Sum),
        vec![Some(0.0), Some(4.0), Some(0.0)]
    );
    assert_eq!(
        align(&buckets, &rows, ChartAggregate::Average),
        vec![None, Some(2.0), None]
    );
}

#[test]
fn top_slices_keeps_the_largest_and_drops_a_real_other_into_the_fold() {
    let rows: Vec<(String, Bucketed)> = [
        ("Fantasy", 10.0),
        ("Horror", 9.0),
        ("Sci-Fi", 8.0),
        ("Crime", 7.0),
        ("Romance", 6.0),
        ("Poetry", 5.0),
        (OTHER_LABEL, 99.0),
    ]
    .into_iter()
    .map(|(name, books)| {
        (
            name.to_string(),
            Bucketed {
                bucket: "2026-01".into(),
                total: books * 100.0,
                n: books,
            },
        )
    })
    .collect();

    let kept = top_slices(&rows);
    assert_eq!(kept.len(), BREAKDOWN_LIMIT - 1);
    // A genre literally named "Other" is folded rather than given its own row
    // beside the synthetic one.
    assert!(!kept.iter().any(|s| s == OTHER_LABEL));
    assert_eq!(kept[0], "Fantasy");
    assert!(!kept.iter().any(|s| s == "Poetry"));
}

// ── End to end, one per grain ───────────────────────────────────────────

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
    )
    .await
    .unwrap();

    // 20% of 300 pages.
    assert_eq!(at(&r, 0, r.buckets.len() - 1), Some(60.0));
}

// ── The cases the design exists for ─────────────────────────────────────

#[tokio::test]
async fn chart_series_aligns_two_different_grains_on_one_bucket_key() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    set_pages(&pool, "uuid-1", 250).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    reading_session(&pool, user, "uuid-2", now, 3_600).await;

    // Completion grain and sitting grain, one chart.
    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::AvgPageLength, ChartMeasure::ReadingMinutes],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();

    let last = r.buckets.len() - 1;
    assert_eq!(at(&r, 0, last), Some(250.0));
    assert_eq!(at(&r, 1, last), Some(60.0));
    // Both series span the same axis, so a caller can zip them positionally.
    assert_eq!(r.series[0].values.len(), r.series[1].values.len());
    assert_eq!(r.series[0].values.len(), r.buckets.len());
}

#[tokio::test]
async fn chart_series_plots_more_measures_than_there_are_axes() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    set_pages(&pool, "uuid-1", 250).await;
    finish_journal(&pool, user, "uuid-1", now).await;
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&pool)
        .await
        .unwrap();
    ledger_day(&pool, user, "uuid-1", &today, 20).await;

    // Three measures, two units: books on the left, both page measures on the
    // right. The axis count is what's bounded, never the measure count.
    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![
                ChartMeasure::BooksFinished,
                ChartMeasure::AvgPageLength,
                ChartMeasure::PagesRead,
            ],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();

    assert_eq!(r.series.len(), 3);
    assert_eq!(r.axes.len(), 2);
    assert_eq!(r.series[0].axis, 0);
    assert_eq!(r.series[1].axis, 1);
    assert_eq!(r.series[2].axis, 1);
    assert_eq!(r.axes[0].unit, ChartUnit::Books);
    assert_eq!(r.axes[1].unit, ChartUnit::Pages);
    // Every series still spans the one shared axis.
    assert!(r.series.iter().all(|s| s.values.len() == r.buckets.len()));
}

#[tokio::test]
async fn chart_series_scales_two_measures_sharing_a_unit_against_one_axis() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 1_800).await;
    listening_session(&pool, user, "uuid-2", now, 600).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![
                ChartMeasure::ReadingMinutes,
                ChartMeasure::ListeningMinutes,
                ChartMeasure::AvgSessionMinutes,
            ],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();

    // All minutes, so one scale — and with one scale there is no second axis
    // to misread a crossing against.
    assert_eq!(r.series.len(), 3);
    assert_eq!(r.axes.len(), 1);
    assert!(r.series.iter().all(|s| s.axis == 0));
}

#[tokio::test]
async fn chart_series_rejects_a_measure_that_would_need_a_third_axis() {
    let (pool, user) = fixture(1).await;
    let err = chart_series(
        &pool,
        user,
        &spec(
            vec![
                ChartMeasure::BooksFinished,
                ChartMeasure::AvgPageLength,
                ChartMeasure::AvgRating,
            ],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        ChartError::Spec(omnibus_shared::ChartSpecError::TooManyUnits(_))
    ));
}

#[tokio::test]
async fn chart_series_opens_a_second_axis_only_for_a_differing_unit() {
    let (pool, user) = fixture(2).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 600).await;
    listening_session(&pool, user, "uuid-2", now, 600).await;

    // Both minutes — one shared axis.
    let same = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::ReadingMinutes, ChartMeasure::ListeningMinutes],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();
    assert_eq!(same.axes.len(), 1);
    assert_eq!(same.series[0].axis, 0);
    assert_eq!(same.series[1].axis, 0);

    // Minutes against sittings — two axes.
    let mixed = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::ReadingMinutes, ChartMeasure::SessionCount],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();
    assert_eq!(mixed.axes.len(), 2);
    assert_eq!(mixed.series[1].axis, 1);
    assert_eq!(mixed.axes[1].unit, ChartUnit::Sessions);
}

#[tokio::test]
async fn chart_series_returns_a_dense_axis_including_buckets_with_no_activity() {
    let (pool, user) = fixture(2).await;
    let three_ago = months_ago_secs(&pool, 3).await;
    let now = months_ago_secs(&pool, 0).await;
    finish_journal(&pool, user, "uuid-1", three_ago).await;
    finish_journal(&pool, user, "uuid-2", now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::AllTime,
        ),
    )
    .await
    .unwrap();

    // The two quiet months between are present, and read zero rather than
    // being dropped — a gap the axis skipped would misreport the trend.
    assert_eq!(r.buckets.len(), 4);
    assert_eq!(
        r.series[0].values,
        vec![Some(1.0), Some(0.0), Some(0.0), Some(1.0)]
    );
}

#[tokio::test]
async fn chart_series_leaves_an_average_absent_in_a_bucket_with_no_data() {
    let (pool, user) = fixture(2).await;
    let two_ago = months_ago_secs(&pool, 2).await;
    let now = months_ago_secs(&pool, 0).await;
    set_pages(&pool, "uuid-1", 400).await;
    set_pages(&pool, "uuid-2", 200).await;
    finish_journal(&pool, user, "uuid-1", two_ago).await;
    finish_journal(&pool, user, "uuid-2", now).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::AvgPageLength],
            ChartBucket::Month,
            StatsRange::AllTime,
        ),
    )
    .await
    .unwrap();

    // The middle month read no books; its average is unknown, never zero.
    assert_eq!(r.series[0].values, vec![Some(400.0), None, Some(200.0)]);
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
    )
    .await
    .unwrap();

    assert!(r.buckets.is_empty());
    assert!(r.series.is_empty());
    assert!(r.is_empty());
    assert!(!r.truncated);
}

#[tokio::test]
async fn chart_series_splits_a_completion_measure_by_genre_and_folds_the_tail() {
    let (pool, user) = fixture(8).await;
    let now = months_ago_secs(&pool, 0).await;
    // Six genres, one book each: five survive, the sixth folds into Other.
    let genres = ["Fantasy", "Horror", "Sci-Fi", "Crime", "Romance", "Poetry"];
    for (i, g) in genres.iter().enumerate() {
        let uuid = format!("uuid-{}", i + 1);
        set_genres(&pool, &uuid, &[g], user).await;
        finish_journal(&pool, user, &uuid, now).await;
    }

    let mut s = spec(
        vec![ChartMeasure::BooksFinished],
        ChartBucket::Month,
        StatsRange::Year,
    );
    s.breakdown = ChartBreakdown::Genre;
    let r = chart_series(&pool, user, &s).await.unwrap();

    assert_eq!(r.series.len(), BREAKDOWN_LIMIT + 1);
    assert_eq!(r.series.last().unwrap().slice.as_deref(), Some(OTHER_LABEL));
    // Every slice shares the measure's unit, so they all sit on one axis.
    assert!(r.series.iter().all(|x| x.axis == 0));
    assert_eq!(r.axes.len(), 1);
}

#[tokio::test]
async fn chart_series_stacks_a_split_count_but_never_a_split_average() {
    let (pool, user) = fixture(4).await;
    let now = months_ago_secs(&pool, 0).await;
    for (i, g) in ["Fantasy", "Horror"].iter().enumerate() {
        let uuid = format!("uuid-{}", i + 1);
        set_pages(&pool, &uuid, 200).await;
        set_genres(&pool, &uuid, &[g], user).await;
        finish_journal(&pool, user, &uuid, now).await;
    }

    // Books finished is a count, so its slices are parts of a whole.
    let mut counted = spec(
        vec![ChartMeasure::BooksFinished],
        ChartBucket::Month,
        StatsRange::Year,
    );
    counted.breakdown = ChartBreakdown::Genre;
    assert!(chart_series(&pool, user, &counted).await.unwrap().stacked);

    // Average book length is a mean, and means do not add.
    let mut averaged = spec(
        vec![ChartMeasure::AvgPageLength],
        ChartBucket::Month,
        StatsRange::Year,
    );
    averaged.breakdown = ChartBreakdown::Genre;
    assert!(!chart_series(&pool, user, &averaged).await.unwrap().stacked);

    // Two separate measures never stack — books on top of pages is not a
    // quantity.
    let unsplit = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBucket::Month,
        StatsRange::Year,
    );
    assert!(!chart_series(&pool, user, &unsplit).await.unwrap().stacked);
}

#[tokio::test]
async fn a_stacked_axis_clears_the_tallest_column_not_the_tallest_slice() {
    let (pool, user) = fixture(6).await;
    let now = months_ago_secs(&pool, 0).await;
    // Six books across three genres, all finished in one month: no slice is
    // taller than two, but the column is six.
    for (i, g) in ["Fantasy", "Fantasy", "Horror", "Horror", "Crime", "Crime"]
        .iter()
        .enumerate()
    {
        let uuid = format!("uuid-{}", i + 1);
        set_genres(&pool, &uuid, &[g], user).await;
        finish_journal(&pool, user, &uuid, now).await;
    }

    let mut s = spec(
        vec![ChartMeasure::BooksFinished],
        ChartBucket::Month,
        StatsRange::Year,
    );
    s.breakdown = ChartBreakdown::Genre;
    let r = chart_series(&pool, user, &s).await.unwrap();

    assert!(r.stacked);
    let last = r.buckets.len() - 1;
    let column: f64 = r.series.iter().filter_map(|x| x.values[last]).sum();
    assert_eq!(column, 6.0);
    // Scaling to the tallest slice would give a 2-high axis and clip every
    // column above it.
    assert!(
        r.axes[0].max >= column,
        "axis {} clips a column of {column}",
        r.axes[0].max
    );
}

#[tokio::test]
async fn chart_series_re_averages_a_folded_genre_slice_from_sums() {
    let (pool, user) = fixture(17).await;
    let now = months_ago_secs(&pool, 0).await;
    // Five three-book genres fill the limit, so the two-book sixth is the one
    // that folds. Its books differ in length, so a fold that averaged the
    // slice averages instead of re-averaging from sums would come out wrong.
    let mut next = 1;
    for g in ["A", "B", "C", "D", "E"] {
        for _ in 0..3 {
            let uuid = format!("uuid-{next}");
            set_pages(&pool, &uuid, 100).await;
            set_genres(&pool, &uuid, &[g], user).await;
            finish_journal(&pool, user, &uuid, now).await;
            next += 1;
        }
    }
    for pages in [200, 400] {
        let uuid = format!("uuid-{next}");
        set_pages(&pool, &uuid, pages).await;
        set_genres(&pool, &uuid, &["Zzz"], user).await;
        finish_journal(&pool, user, &uuid, now).await;
        next += 1;
    }

    let mut s = spec(
        vec![ChartMeasure::AvgPageLength],
        ChartBucket::Month,
        StatsRange::Year,
    );
    s.breakdown = ChartBreakdown::Genre;
    let r = chart_series(&pool, user, &s).await.unwrap();

    let other = r
        .series
        .iter()
        .find(|x| x.slice.as_deref() == Some(OTHER_LABEL))
        .expect("a folded tail");
    // (200 + 400) / 2 books.
    assert_eq!(other.values[r.buckets.len() - 1], Some(300.0));
}

#[tokio::test]
async fn chart_series_carries_a_bounded_measures_caveat_into_the_result() {
    let (pool, user) = fixture(1).await;
    let now = months_ago_secs(&pool, 0).await;
    reading_session(&pool, user, "uuid-1", now, 600).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::PagesRead, ChartMeasure::ReadingMinutes],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap();

    assert_eq!(r.caveats.len(), 1);
    assert!(r.caveats[0].starts_with("Pages read is measured"));
}

#[tokio::test]
async fn chart_series_rejects_an_invalid_spec_before_running_any_sql() {
    let (pool, user) = fixture(1).await;
    let err = chart_series(
        &pool,
        user,
        &spec(vec![], ChartBucket::Month, StatsRange::Year),
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        ChartError::Spec(omnibus_shared::ChartSpecError::NoMeasures)
    ));
}

#[tokio::test]
async fn chart_series_propagates_a_db_error_when_the_pool_is_closed() {
    let (pool, user) = fixture(1).await;
    pool.close().await;

    let err = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, ChartError::Stats(_)));
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
    )
    .await
    .unwrap();

    assert!(r.is_empty());
}

#[tokio::test]
async fn chart_series_clips_a_long_axis_to_the_most_recent_buckets_and_says_so() {
    let (pool, user) = fixture(1).await;
    // One session far enough back that a daily axis exceeds the cap.
    let long_ago = MAX_BUCKETS as i64 + 40;
    let start: i64 = sqlx::query_scalar(&format!(
        "SELECT CAST(strftime('%s', date('now', '-{long_ago} days')) AS INTEGER)"
    ))
    .fetch_one(&pool)
    .await
    .unwrap();
    reading_session(&pool, user, "uuid-1", start, 600).await;
    reading_session(&pool, user, "uuid-1", start + 10 * DAY, 600).await;

    let r = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::ReadingMinutes],
            ChartBucket::Day,
            StatsRange::AllTime,
        ),
    )
    .await
    .unwrap();

    assert!(r.truncated);
    assert_eq!(r.buckets.len(), MAX_BUCKETS);
    // The clip keeps the newest end, so the last bucket is still today.
    let today: String = sqlx::query_scalar("SELECT date('now')")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(r.buckets.last().unwrap(), &today);
}
