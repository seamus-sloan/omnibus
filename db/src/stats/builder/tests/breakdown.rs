//! The genre breakdown: a completion measure split by genre with the tail
//! folded, a split count stacked but never a split average, the stacked
//! axis clearing the tallest column, and a folded slice re-averaged from
//! sums.

use omnibus_shared::{ChartBreakdown, StatsRange};

use super::super::*;
use super::{fixture, set_pages, spec};
use crate::stats::tests::{finish_journal, months_ago_secs, set_genres};

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
    let r = chart_series(&pool, user, &s, None).await.unwrap();

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
    assert!(
        chart_series(&pool, user, &counted, None)
            .await
            .unwrap()
            .stacked
    );

    // Average book length is a mean, and means do not add.
    let mut averaged = spec(
        vec![ChartMeasure::AvgPageLength],
        ChartBucket::Month,
        StatsRange::Year,
    );
    averaged.breakdown = ChartBreakdown::Genre;
    assert!(
        !chart_series(&pool, user, &averaged, None)
            .await
            .unwrap()
            .stacked
    );

    // Two separate measures never stack — books on top of pages is not a
    // quantity.
    let unsplit = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBucket::Month,
        StatsRange::Year,
    );
    assert!(
        !chart_series(&pool, user, &unsplit, None)
            .await
            .unwrap()
            .stacked
    );
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
    let r = chart_series(&pool, user, &s, None).await.unwrap();

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
    let r = chart_series(&pool, user, &s, None).await.unwrap();

    let other = r
        .series
        .iter()
        .find(|x| x.slice.as_deref() == Some(OTHER_LABEL))
        .expect("a folded tail");
    // (200 + 400) / 2 books.
    assert_eq!(other.values[r.buckets.len() - 1], Some(300.0));
}
