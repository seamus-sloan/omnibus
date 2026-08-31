//! Tests for the chart renderer: the pure label/geometry helpers, and a
//! rendered pass asserting the marks and the accessible table actually land.

use omnibus_shared::{ChartAxis, ChartMeasure, ChartUnit};

use super::*;
use crate::test_support::render_in_vdom;

fn series(
    measure: ChartMeasure,
    axis: u8,
    mark: ChartMark,
    values: Vec<Option<f64>>,
) -> ChartSeries {
    ChartSeries {
        measure,
        slice: None,
        axis,
        mark,
        values,
    }
}

/// Books finished (bars, left) against average length (line, right) — the
/// comparison the builder exists for, with a gap in the average.
fn sample() -> ChartResult {
    ChartResult {
        bucket: ChartBucket::Month,
        buckets: vec![
            "2026-01".into(),
            "2026-02".into(),
            "2026-03".into(),
            "2026-04".into(),
        ],
        series: vec![
            series(
                ChartMeasure::BooksFinished,
                0,
                ChartMark::Bar,
                vec![Some(2.0), Some(1.0), Some(0.0), Some(3.0)],
            ),
            // A run of two, then a gap — so the renderer has both a segment
            // to draw and a break to honour.
            series(
                ChartMeasure::AvgPageLength,
                1,
                ChartMark::Line,
                vec![Some(320.0), Some(280.0), None, Some(180.0)],
            ),
        ],
        axes: vec![
            ChartAxis {
                unit: ChartUnit::Books,
                max: 5.0,
            },
            ChartAxis {
                unit: ChartUnit::Pages,
                max: 500.0,
            },
        ],
        divisions: 4,
        truncated: false,
        caveats: vec![],
    }
}

// ── Labels ──────────────────────────────────────────────────────────────

#[test]
fn bucket_label_renders_each_granularity_in_its_own_shape() {
    assert_eq!(bucket_label("2026", ChartBucket::Year), "2026");
    assert_eq!(bucket_label("2026-03", ChartBucket::Month), "Mar");
    // January carries the year so a multi-year axis stays readable.
    assert_eq!(bucket_label("2026-01", ChartBucket::Month), "Jan 26");
    assert_eq!(bucket_label("2026-03-14", ChartBucket::Day), "14 Mar");
    assert_eq!(bucket_label("2026-03-09", ChartBucket::Week), "9 Mar");
}

#[test]
fn bucket_label_falls_back_to_the_raw_key_when_it_cannot_parse_one() {
    assert_eq!(bucket_label("nonsense", ChartBucket::Month), "nonsense");
    assert_eq!(bucket_label("2026-13", ChartBucket::Month), "2026-13");
    assert_eq!(bucket_label("2026-99-01", ChartBucket::Day), "2026-99-01");
}

#[test]
fn tick_label_drops_precision_the_magnitude_does_not_deserve() {
    assert_eq!(tick_label(0.0), "0");
    assert_eq!(tick_label(4.5), "4.5");
    assert_eq!(tick_label(12.5), "12");
    assert_eq!(tick_label(500.0), "500");
    assert_eq!(tick_label(317.4), "317");
}

#[test]
fn value_label_rounds_a_near_integer_but_keeps_a_real_fraction() {
    assert_eq!(value_label(3.0), "3");
    assert_eq!(value_label(3.02), "3");
    assert_eq!(value_label(4.5), "4.5");
    assert_eq!(value_label(320.0), "320");
}

// ── Geometry ────────────────────────────────────────────────────────────

#[test]
fn y_maps_zero_to_the_baseline_and_the_axis_max_to_the_top() {
    let plot = Plot::new(&sample());
    assert!((plot.y(0.0, 5.0) - plot.baseline()).abs() < 0.001);
    assert!((plot.y(5.0, 5.0) - PAD_T).abs() < 0.001);
}

#[test]
fn y_clamps_a_value_that_overshoots_its_axis_into_the_frame() {
    let plot = Plot::new(&sample());
    assert!((plot.y(50.0, 5.0) - PAD_T).abs() < 0.001);
    assert!((plot.y(-10.0, 5.0) - plot.baseline()).abs() < 0.001);
}

#[test]
fn y_stays_on_the_baseline_rather_than_dividing_by_a_zero_axis() {
    let plot = Plot::new(&sample());
    assert!((plot.y(3.0, 0.0) - plot.baseline()).abs() < 0.001);
}

#[test]
fn a_second_axis_widens_the_right_padding_to_make_room_for_its_labels() {
    let single = ChartResult {
        axes: vec![ChartAxis {
            unit: ChartUnit::Books,
            max: 5.0,
        }],
        ..sample()
    };
    assert_eq!(Plot::new(&single).pad_r, PAD_R_SINGLE);
    assert_eq!(Plot::new(&sample()).pad_r, PAD_R_DUAL);
}

#[test]
fn band_centres_are_evenly_spaced_across_the_plot_area() {
    let plot = Plot::new(&sample());
    let gap = plot.band_centre(1) - plot.band_centre(0);
    assert!((gap - plot.band_w()).abs() < 0.001);
    assert!(plot.band_centre(0) > PAD_L);
    assert!(plot.band_centre(2) < VIEW_W - plot.pad_r);
}

#[test]
fn axis_max_falls_back_to_the_left_axis_when_a_series_names_a_missing_one() {
    let mut result = sample();
    result.axes.truncate(1);
    // The second series still claims axis 1, which no longer exists.
    assert_eq!(axis_max(&result, &result.series[1]), 5.0);
}

// ── Line runs ───────────────────────────────────────────────────────────

#[test]
fn line_runs_breaks_the_line_at_a_bucket_with_no_data() {
    let result = ChartResult {
        buckets: vec!["a".into(), "b".into(), "c".into(), "d".into()],
        ..sample()
    };
    let plot = Plot::new(&result);
    let s = series(
        ChartMeasure::AvgPageLength,
        0,
        ChartMark::Line,
        vec![Some(1.0), Some(2.0), None, Some(3.0)],
    );
    let runs = line_runs(&plot, &s, 5.0);
    // Two points then a gap: one run, and the lone trailing point is not a
    // second one — a segment needs two ends.
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].split(' ').count(), 2);
}

#[test]
fn line_runs_returns_nothing_for_a_series_that_never_has_two_adjacent_points() {
    let plot = Plot::new(&sample());
    let s = series(
        ChartMeasure::AvgPageLength,
        0,
        ChartMark::Line,
        vec![Some(1.0), None, Some(3.0)],
    );
    assert!(line_runs(&plot, &s, 5.0).is_empty());
}

#[test]
fn line_runs_joins_a_fully_populated_series_into_one_segment() {
    let plot = Plot::new(&sample());
    let s = series(
        ChartMeasure::AvgPageLength,
        0,
        ChartMark::Line,
        vec![Some(1.0), Some(2.0), Some(3.0)],
    );
    let runs = line_runs(&plot, &s, 5.0);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].split(' ').count(), 3);
}

// ── Rendered output ─────────────────────────────────────────────────────

#[test]
fn the_plot_draws_bars_for_a_count_and_a_polyline_for_an_average() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: sample() }
        }
    });
    assert!(html.contains("<rect"), "expected bars: {html}");
    assert!(html.contains("<polyline"), "expected a line: {html}");
    // One dot per present average — the absent bucket contributes none.
    assert_eq!(html.matches("<circle").count(), 3);
}

#[test]
fn the_plot_draws_the_gridline_count_the_result_names() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot {
                result: ChartResult { divisions: 3, ..sample() }
            }
        }
    });
    // Three divisions means four gridlines, plus the baseline.
    assert_eq!(html.matches("cb-grid").count(), 4);
}

#[test]
fn the_plot_labels_both_axes_when_the_result_carries_two() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: sample() }
        }
    });
    assert!(html.contains("cb-tick-left"));
    assert!(html.contains("cb-tick-right"));
    // The right axis tops out at its own maximum, not the left's.
    assert!(
        html.contains(">500<"),
        "expected the right axis max: {html}"
    );
}

#[test]
fn the_plot_omits_the_right_axis_when_there_is_only_one() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot {
                result: ChartResult {
                    series: vec![series(
                        ChartMeasure::BooksFinished,
                        0,
                        ChartMark::Bar,
                        vec![Some(2.0), Some(1.0), Some(0.0), Some(3.0)],
                    )],
                    axes: vec![ChartAxis { unit: ChartUnit::Books, max: 5.0 }],
                    ..sample()
                }
            }
        }
    });
    assert!(!html.contains("cb-tick-right"));
}

#[test]
fn the_table_carries_every_value_the_chart_plots() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartTable { result: sample() }
        }
    });
    assert!(html.contains("cb-table"));
    assert!(html.contains(">320<"), "expected a plotted value: {html}");
    // The absent average says so rather than reading as a zero.
    assert!(
        html.contains("no data"),
        "expected the absent bucket: {html}"
    );
}

#[test]
fn the_legend_names_each_series_and_the_axis_it_is_scaled_against() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartLegend { result: sample() }
        }
    });
    assert!(html.contains("Books finished"));
    assert!(html.contains("Avg book length"));
    assert!(html.contains(">left<"));
    assert!(html.contains(">right<"));
}

#[test]
fn the_legend_drops_the_axis_marker_on_a_single_axis_chart() {
    let html = render_in_vdom(|| {
        rsx! {
        ChartLegend {
            result: ChartResult {
                series: vec![series(
                    ChartMeasure::BooksFinished,
                    0,
                    ChartMark::Bar,
                    vec![Some(2.0)],
                )],
                axes: vec![ChartAxis { unit: ChartUnit::Books, max: 5.0 }],
                ..sample()
            }
        }
        }
    });
    assert!(!html.contains(">left<"));
}

#[test]
fn the_legend_qualifies_a_split_series_with_its_slice() {
    let html = render_in_vdom(|| {
        rsx! {
        ChartLegend {
            result: ChartResult {
                series: vec![ChartSeries {
                    slice: Some("Fantasy".into()),
                    ..series(ChartMeasure::BooksFinished, 0, ChartMark::Bar, vec![Some(2.0)])
                }],
                axes: vec![ChartAxis { unit: ChartUnit::Books, max: 5.0 }],
                ..sample()
            }
        }
        }
    });
    assert!(html.contains("Books finished · Fantasy"));
}
