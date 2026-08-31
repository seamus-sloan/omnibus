//! Tests for the client-side zoom: resolving a key range, slicing to it, and
//! what a brush counts as a selection.

use omnibus_shared::{ChartAxis, ChartBucket, ChartMark, ChartMeasure, ChartSeries, ChartUnit};

use super::*;

fn months(n: usize) -> Vec<String> {
    (1..=n).map(|m| format!("2026-{m:02}")).collect()
}

fn result(values: Vec<Option<f64>>) -> ChartResult {
    let n = values.len();
    ChartResult {
        bucket: ChartBucket::Month,
        buckets: months(n),
        series: vec![ChartSeries {
            measure: ChartMeasure::BooksFinished,
            slice: None,
            axis: 0,
            mark: ChartMark::Bar,
            values,
        }],
        axes: vec![ChartAxis {
            unit: ChartUnit::Books,
            max: 100.0,
        }],
        divisions: 4,
        stacked: false,
        truncated: false,
        caveats: vec![],
    }
}

fn key(m: usize) -> String {
    format!("2026-{m:02}")
}

// ── Resolving ───────────────────────────────────────────────────────────

#[test]
fn resolve_finds_an_inclusive_index_span() {
    let b = months(8);
    assert_eq!(resolve(&b, &(key(2), key(5))), Some((1, 4)));
}

#[test]
fn resolve_orders_a_backwards_drag() {
    // Brushing right-to-left is the same selection.
    let b = months(8);
    assert_eq!(resolve(&b, &(key(5), key(2))), Some((1, 4)));
}

#[test]
fn resolve_drops_a_zoom_whose_keys_have_left_the_axis() {
    // Regrouping monthly to weekly replaces every key; an index-based zoom
    // would silently frame a different stretch of the reader's history.
    let weekly = vec!["2026-03-02".to_string(), "2026-03-09".to_string()];
    assert_eq!(resolve(&weekly, &(key(2), key(5))), None);

    // One end surviving is not enough either.
    let b = months(3);
    assert_eq!(resolve(&b, &(key(2), key(9))), None);
}

#[test]
fn resolve_refuses_a_single_bucket_span() {
    let b = months(8);
    assert_eq!(resolve(&b, &(key(3), key(3))), None);
}

// ── Slicing ─────────────────────────────────────────────────────────────

#[test]
fn apply_narrows_the_buckets_and_every_series_together() {
    let r = result((1..=8).map(|v| Some(f64::from(v))).collect());
    let z = apply(&r, Some(&(key(3), key(5))));
    assert_eq!(z.buckets, vec![key(3), key(4), key(5)]);
    assert_eq!(z.series[0].values, vec![Some(3.0), Some(4.0), Some(5.0)]);
}

#[test]
fn apply_refits_the_axis_to_what_is_on_screen() {
    // A quiet stretch inside a tall range: unzoomed it is flat against the
    // floor, and re-fitting is the whole point of zooming into it.
    let mut values: Vec<Option<f64>> = (1..=8).map(|_| Some(2.0)).collect();
    values[7] = Some(400.0);
    let r = {
        let mut r = result(values);
        r.axes[0].max = 400.0;
        r
    };
    let z = apply(&r, Some(&(key(1), key(4))));
    assert!(
        z.axes[0].max < 20.0,
        "axis stayed at {} for a slice topping out at 2",
        z.axes[0].max
    );
}

#[test]
fn apply_returns_the_whole_result_when_the_range_does_not_resolve() {
    let r = result(vec![Some(1.0), Some(2.0), Some(3.0)]);
    // A stale zoom degrades to the full view rather than to an error.
    let z = apply(&r, Some(&("nope".into(), "also-nope".into())));
    assert_eq!(z.buckets, r.buckets);
    let none = apply(&r, None);
    assert_eq!(none.buckets, r.buckets);
}

#[test]
fn apply_keeps_the_truncation_flag_since_it_describes_the_fetch() {
    // The clip is a fact about what was fetched, not about this view, so it
    // is still reported while zoomed.
    let mut r = result(vec![Some(1.0), Some(2.0), Some(3.0)]);
    r.truncated = true;
    assert!(apply(&r, Some(&(key(1), key(2)))).truncated);
}

// ── Brushing ────────────────────────────────────────────────────────────

#[test]
fn brush_range_needs_more_than_one_bucket() {
    let b = months(8);
    // A click, not a drag.
    assert_eq!(brush_range(&b, 3, 3), None);
    assert_eq!(brush_range(&b, 3, 4), Some((key(4), key(5))));
}

#[test]
fn brush_range_normalises_a_right_to_left_drag() {
    let b = months(8);
    assert_eq!(brush_range(&b, 5, 1), Some((key(2), key(6))));
}

#[test]
fn brush_range_is_none_when_an_index_is_off_the_axis() {
    let b = months(3);
    assert_eq!(brush_range(&b, 1, 9), None);
}
