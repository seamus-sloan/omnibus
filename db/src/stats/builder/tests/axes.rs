//! Axes and the result shape: mixed grains on one bucket key, more
//! measures than axes, a shared unit on one axis and a second axis only
//! for a differing unit, the dense axis with absent averages, the clipped
//! long axis, the bounded-measures caveat, the empty selection, and the
//! forbidden-spec and DB-failure rejections.

use omnibus_shared::StatsRange;

use super::super::*;
use super::{at, fixture, ledger_day, set_pages, spec};
use crate::stats::tests::{
    finish_journal, listening_session, months_ago_secs, reading_session, DAY,
};

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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
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
        None,
    )
    .await
    .unwrap();

    // The middle month read no books; its average is unknown, never zero.
    assert_eq!(r.series[0].values, vec![Some(400.0), None, Some(200.0)]);
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
        None,
    )
    .await
    .unwrap();

    assert_eq!(r.caveats.len(), 1);
    assert!(r.caveats[0].starts_with("Pages read is measured"));
}

#[tokio::test]
async fn chart_series_answers_an_empty_selection_without_touching_the_db() {
    let (pool, user) = fixture(1).await;
    // Closed pool: any query at all would fail, so this passing is the proof
    // that an empty selection short-circuits before the fan-out.
    pool.close().await;

    let r = chart_series(
        &pool,
        user,
        &spec(vec![], ChartBucket::Month, StatsRange::Year),
        None,
    )
    .await
    .unwrap();

    assert!(r.is_empty());
    assert!(r.series.is_empty());
    assert!(r.buckets.is_empty());
    // The bucket still comes back, so a surface can say what *would* be drawn.
    assert_eq!(r.bucket, ChartBucket::Month);
}

#[tokio::test]
async fn chart_series_rejects_a_spec_the_vocabulary_forbids() {
    let (pool, user) = fixture(1).await;
    let err = chart_series(
        &pool,
        user,
        &spec(
            vec![ChartMeasure::BooksFinished, ChartMeasure::BooksFinished],
            ChartBucket::Month,
            StatsRange::Year,
        ),
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(
        err,
        ChartError::Spec(omnibus_shared::ChartSpecError::DuplicateMeasures(_))
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
        None,
    )
    .await
    .unwrap_err();

    assert!(matches!(err, ChartError::Stats(_)));
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
        None,
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
