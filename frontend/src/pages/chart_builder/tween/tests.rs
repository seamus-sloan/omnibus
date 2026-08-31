//! Tests for the tween: which changes count as an update, and what a blended
//! frame holds.

use omnibus_shared::{ChartAxis, ChartBucket, ChartMeasure, ChartSeries, ChartUnit};

use super::*;

fn series(measure: ChartMeasure, slice: Option<&str>, values: Vec<Option<f64>>) -> ChartSeries {
    ChartSeries {
        measure,
        slice: slice.map(str::to_string),
        axis: 0,
        mark: measure.mark(),
        values,
    }
}

fn result(buckets: &[&str], series: Vec<ChartSeries>, max: f64) -> ChartResult {
    ChartResult {
        bucket: ChartBucket::Month,
        buckets: buckets.iter().map(|b| (*b).to_string()).collect(),
        series,
        axes: vec![ChartAxis {
            unit: ChartUnit::Books,
            max,
        }],
        divisions: 4,
        stacked: false,
        truncated: false,
        caveats: vec![],
    }
}

fn two_buckets(values: Vec<Option<f64>>, max: f64) -> ChartResult {
    result(
        &["2026-01", "2026-02"],
        vec![series(ChartMeasure::BooksFinished, None, values)],
        max,
    )
}

// ── Blending ────────────────────────────────────────────────────────────

#[test]
fn a_blend_starts_at_the_old_values_and_ends_at_the_new() {
    let a = two_buckets(vec![Some(0.0), Some(10.0)], 20.0);
    let b = two_buckets(vec![Some(10.0), Some(0.0)], 20.0);

    let start = blend(&a, &b, 0.0);
    assert_eq!(start.series[0].values, vec![Some(0.0), Some(10.0)]);

    let end = blend(&a, &b, 1.0);
    assert_eq!(end.series[0].values, vec![Some(10.0), Some(0.0)]);
}

#[test]
fn a_mid_blend_lies_between_the_two_and_moves_each_value_its_own_way() {
    let a = two_buckets(vec![Some(0.0), Some(10.0)], 20.0);
    let b = two_buckets(vec![Some(10.0), Some(0.0)], 20.0);
    let mid = blend(&a, &b, 0.5);

    let rising = mid.series[0].values[0].unwrap();
    let falling = mid.series[0].values[1].unwrap();
    assert!(rising > 0.0 && rising < 10.0, "{rising}");
    assert!(falling > 0.0 && falling < 10.0, "{falling}");
    // Eased, so the pair is symmetric about the midpoint but not at it.
    assert!((rising + falling - 10.0).abs() < 0.001);
}

#[test]
fn the_axis_is_already_the_new_one_at_every_frame() {
    // Lerping the maximum would drag the tick labels through 3.7 and 7.4;
    // they are meant to be round, so the scale changes at once.
    let a = two_buckets(vec![Some(1.0)], 5.0);
    let b = two_buckets(vec![Some(9.0)], 10.0);
    for t in [0.0, 0.25, 0.5, 1.0] {
        assert_eq!(blend(&a, &b, t).axes[0].max, 10.0, "at t={t}");
    }
}

#[test]
fn a_mark_with_no_counterpart_grows_in_from_the_baseline() {
    // Unmeasured before, present now: it enters, and an entering mark rises
    // from zero rather than appearing at full height.
    let a = two_buckets(vec![None, Some(4.0)], 10.0);
    let b = two_buckets(vec![Some(8.0), Some(8.0)], 10.0);
    let mid = blend(&a, &b, 0.5);
    let entering = mid.series[0].values[0].unwrap();
    assert!(entering > 0.0 && entering < 8.0, "{entering}");
    let travelling = mid.series[0].values[1].unwrap();
    assert!(travelling > 4.0 && travelling < 8.0, "{travelling}");
}

#[test]
fn values_are_matched_by_bucket_key_not_by_position() {
    // The window slid on by a month: February is index 1 before and index 0
    // after. Matching positionally would animate January's value into
    // February's slot — a number that never existed.
    let a = result(
        &["2026-01", "2026-02"],
        vec![series(
            ChartMeasure::BooksFinished,
            None,
            vec![Some(0.0), Some(10.0)],
        )],
        20.0,
    );
    let b = result(
        &["2026-02", "2026-03"],
        vec![series(
            ChartMeasure::BooksFinished,
            None,
            vec![Some(10.0), Some(6.0)],
        )],
        20.0,
    );
    let mid = blend(&a, &b, 0.5);
    // February held 10 before and holds 10 now, so it does not move at all.
    assert_eq!(mid.series[0].values[0], Some(10.0));
    // March is new, so it rises from the floor.
    let entering = mid.series[0].values[1].unwrap();
    assert!(entering > 0.0 && entering < 6.0, "{entering}");
}

#[test]
fn a_zoom_keeps_its_surviving_buckets_still() {
    // Zooming is a subset of the same keys, so every surviving bar is already
    // at its value and only the scale around it changes.
    let full = result(
        &["2026-01", "2026-02", "2026-03", "2026-04"],
        vec![series(
            ChartMeasure::BooksFinished,
            None,
            vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)],
        )],
        10.0,
    );
    let zoomed = result(
        &["2026-02", "2026-03"],
        vec![series(
            ChartMeasure::BooksFinished,
            None,
            vec![Some(2.0), Some(3.0)],
        )],
        4.0,
    );
    let mid = blend(&full, &zoomed, 0.5);
    assert_eq!(mid.series[0].values, vec![Some(2.0), Some(3.0)]);
}

#[test]
fn a_series_matches_on_its_slice_so_genres_do_not_cross_animate() {
    let a = result(
        &["2026-01"],
        vec![
            series(
                ChartMeasure::BooksFinished,
                Some("Fantasy"),
                vec![Some(4.0)],
            ),
            series(ChartMeasure::BooksFinished, Some("Horror"), vec![Some(0.0)]),
        ],
        10.0,
    );
    // The two slices swap order on the wire; matching by position would
    // animate Fantasy's value into Horror's bar.
    let b = result(
        &["2026-01"],
        vec![
            series(ChartMeasure::BooksFinished, Some("Horror"), vec![Some(0.0)]),
            series(
                ChartMeasure::BooksFinished,
                Some("Fantasy"),
                vec![Some(4.0)],
            ),
        ],
        10.0,
    );
    let mid = blend(&a, &b, 0.5);
    assert_eq!(mid.series[0].values, vec![Some(0.0)]);
    assert_eq!(mid.series[1].values, vec![Some(4.0)]);
}

#[test]
fn a_blend_carries_the_targets_flags_rather_than_the_old_ones() {
    let a = two_buckets(vec![Some(1.0), Some(2.0)], 4.0);
    let mut b = two_buckets(vec![Some(3.0), Some(4.0)], 4.0);
    b.stacked = true;
    b.truncated = true;
    b.caveats = vec!["A caveat.".into()];

    let mid = blend(&a, &b, 0.5);
    assert!(mid.stacked);
    assert!(mid.truncated);
    assert_eq!(mid.caveats, vec!["A caveat.".to_string()]);
}

#[test]
fn ease_out_is_monotonic_and_pinned_at_both_ends() {
    assert_eq!(ease_out(0.0), 0.0);
    assert_eq!(ease_out(1.0), 1.0);
    // Out-eased, so it is past halfway at the halfway point.
    assert!(ease_out(0.5) > 0.5);
    let mut last = 0.0;
    for i in 0..=20 {
        let v = ease_out(f64::from(i) / 20.0);
        assert!(v >= last, "not monotonic at {i}");
        last = v;
    }
    // Clamped, so a stray frame past the end can't overshoot.
    assert_eq!(ease_out(1.4), 1.0);
    assert_eq!(ease_out(-0.2), 0.0);
}
