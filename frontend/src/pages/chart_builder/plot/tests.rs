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
        stacked: false,
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
    assert!(runs[0].path.starts_with('M'));
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
    // Three points curve, so the path carries cubic segments and its area is
    // the same path closed to the baseline.
    assert!(runs[0].path.contains(" C"), "{}", runs[0].path);
    assert!(runs[0].area.ends_with('Z'), "{}", runs[0].area);
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
    assert!(html.contains("cb-stroke"), "expected a line: {html}");
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

// ── Curve ───────────────────────────────────────────────────────────────

#[test]
fn monotone_path_is_empty_below_two_points_and_straight_at_two() {
    assert_eq!(monotone_path(&[]), "");
    assert_eq!(monotone_path(&[Pt { x: 0.0, y: 0.0 }]), "");
    let two = monotone_path(&[Pt { x: 0.0, y: 10.0 }, Pt { x: 10.0, y: 0.0 }]);
    assert_eq!(two, "M0.0,10.0 L10.0,0.0");
}

#[test]
fn monotone_path_never_overshoots_the_values_it_passes_through() {
    // A spike: a cardinal spline would bulge below y=0 on the way back up,
    // drawing a value lower than anything recorded. Monotone tangents go flat
    // at the extremum instead.
    let pts = [
        Pt { x: 0.0, y: 100.0 },
        Pt { x: 10.0, y: 100.0 },
        Pt { x: 20.0, y: 0.0 },
        Pt { x: 30.0, y: 100.0 },
        Pt { x: 40.0, y: 100.0 },
    ];
    let d = monotone_path(&pts);
    // Every control-point y in the path stays inside the data's own range.
    let ys: Vec<f64> = d
        .split([' ', 'M', 'C'])
        .filter(|t| t.contains(','))
        .filter_map(|t| t.split(',').nth(1))
        .filter_map(|v| v.parse::<f64>().ok())
        .collect();
    assert!(!ys.is_empty());
    for y in ys {
        assert!(
            (-0.01..=100.01).contains(&y),
            "control point {y} escaped the data range"
        );
    }
}

#[test]
fn monotone_path_stays_flat_through_a_flat_run() {
    let pts = [
        Pt { x: 0.0, y: 50.0 },
        Pt { x: 10.0, y: 50.0 },
        Pt { x: 20.0, y: 50.0 },
    ];
    let d = monotone_path(&pts);
    assert!(!d.contains("49."), "{d}");
    assert!(!d.contains("51."), "{d}");
}

// ── Stacking ────────────────────────────────────────────────────────────

fn stacked_sample() -> ChartResult {
    ChartResult {
        stacked: true,
        series: vec![
            ChartSeries {
                slice: Some("Fantasy".into()),
                ..series(
                    ChartMeasure::BooksFinished,
                    0,
                    ChartMark::Bar,
                    vec![Some(2.0), Some(1.0), Some(0.0), Some(3.0)],
                )
            },
            ChartSeries {
                slice: Some("Horror".into()),
                ..series(
                    ChartMeasure::BooksFinished,
                    0,
                    ChartMark::Bar,
                    vec![Some(1.0), Some(2.0), Some(1.0), None],
                )
            },
        ],
        axes: vec![ChartAxis {
            unit: ChartUnit::Books,
            max: 6.0,
        }],
        ..sample()
    }
}

#[test]
fn stack_offsets_put_each_series_on_the_running_total_beneath_it() {
    let r = stacked_sample();
    let offsets = stack_offsets(&r, &[0, 1]);
    // The first series always starts at zero.
    assert_eq!(offsets[0], vec![0.0, 0.0, 0.0, 0.0]);
    // The second starts on the first's values — and an absent value in the
    // series above simply contributes nothing rather than leaving a hole.
    assert_eq!(offsets[1], vec![2.0, 1.0, 0.0, 3.0]);
}

#[test]
fn a_stacked_chart_puts_every_bar_in_one_lane_per_bucket() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: stacked_sample() }
        }
    });
    // Two series, four buckets, one absent value — seven bars, all sharing a
    // lane, so each bucket's x positions collapse to a single value.
    let xs: Vec<&str> = html
        .match_indices("<rect class=\"cb-bar\"")
        .filter_map(|(i, _)| html[i..].split("x=\"").nth(1))
        .filter_map(|t| t.split('"').next())
        .collect();
    let distinct: std::collections::BTreeSet<&str> = xs.iter().copied().collect();
    assert_eq!(distinct.len(), 4, "expected one lane per bucket: {xs:?}");
}

/// The same two series, side by side instead of stacked.
fn grouped_sample() -> ChartResult {
    ChartResult {
        stacked: false,
        ..stacked_sample()
    }
}

#[test]
fn a_grouped_chart_gives_each_bar_series_its_own_lane() {
    // `render_in_vdom` takes a plain `fn`, so the fixture is a function rather
    // than a captured local.
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: grouped_sample() }
        }
    });
    let xs: Vec<&str> = html
        .match_indices("<rect class=\"cb-bar\"")
        .filter_map(|(i, _)| html[i..].split("x=\"").nth(1))
        .filter_map(|t| t.split('"').next())
        .collect();
    let distinct: std::collections::BTreeSet<&str> = xs.iter().copied().collect();
    // Grouped lanes never collide, so every drawn bar has its own x: four
    // from the first series, three from the second (which is absent in one
    // bucket). Stacked, the same data collapses to four.
    assert_eq!(xs.len(), 7, "{xs:?}");
    assert_eq!(distinct.len(), 7, "expected a lane per series: {xs:?}");
}

// ── Hover ───────────────────────────────────────────────────────────────

#[test]
fn the_plot_renders_a_hit_target_for_every_bucket() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: sample() }
        }
    });
    assert_eq!(html.matches("cb-hit").count(), 4);
}

#[test]
fn the_plot_starts_with_no_hover_so_ssr_and_first_paint_agree() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartPlot { result: sample() }
        }
    });
    // Rule 07: the card and the band are client-only state, so neither may
    // appear in the server-rendered markup.
    assert!(!html.contains("chart-tooltip"), "{html}");
    assert!(!html.contains("cb-band"), "{html}");
}

#[test]
fn the_hover_card_reads_every_series_at_the_hovered_bucket() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: sample(), index: 0 }
        }
    });
    assert!(
        html.contains("Jan 2026"),
        "expected a full bucket name: {html}"
    );
    assert!(html.contains("Books finished"));
    assert!(html.contains("Avg book length"));
    // Values carry their unit, since two series can be on different scales.
    assert!(html.contains("2 books"), "{html}");
    assert!(html.contains("320 pages"), "{html}");
}

#[test]
fn the_hover_card_says_no_data_rather_than_zero_for_an_absent_average() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: sample(), index: 2 }
        }
    });
    assert!(html.contains("no data"), "{html}");
}

#[test]
fn the_hover_card_flips_past_the_midpoint_so_it_stays_on_the_plot() {
    let near = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: sample(), index: 0 }
        }
    });
    assert!(!near.contains("is-flipped"), "{near}");

    let far = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: sample(), index: 3 }
        }
    });
    assert!(far.contains("is-flipped"), "{far}");
}

#[test]
fn the_hover_card_renders_nothing_for_a_bucket_outside_the_axis() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: sample(), index: 99 }
        }
    });
    assert!(!html.contains("chart-tooltip"), "{html}");
}

#[test]
fn the_hover_card_drops_a_stacked_slice_that_draws_no_segment() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: stacked_sample(), index: 2 }
        }
    });
    // Fantasy is zero in that bucket, so it contributes no segment and no row.
    assert!(html.contains("Horror"), "{html}");
    assert!(!html.contains("Fantasy"), "{html}");
}

#[test]
fn the_hover_card_keeps_a_real_zero_when_the_chart_is_not_stacked() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartHoverCard { result: grouped_sample(), index: 2 }
        }
    });
    // Unstacked, a zero is the measure's own value and stays on the card.
    assert!(html.contains("Fantasy"), "{html}");
    assert!(html.contains("0 books"), "{html}");
}
