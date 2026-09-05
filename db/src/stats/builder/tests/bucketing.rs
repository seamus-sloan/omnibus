//! The pure bucketing and alignment helpers: `bucket_expr` per grain and
//! its inverse, whole-number gridlines on a count axis, `reduce`
//! re-averaging from row counts, `align` zero-filling totals only, and
//! `top_slices` folding the tail into a real Other.

use omnibus_shared::StatsRange;

use super::super::*;
use super::{fixture, spec};
use crate::stats::tests::{finish_journal, months_ago_secs};

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
        None,
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
