use super::*;
use crate::stats::StatsRange;

fn spec(measures: Vec<ChartMeasure>, breakdown: ChartBreakdown) -> ChartSpec {
    ChartSpec {
        measures,
        bucket: ChartBucket::Month,
        range: StatsRange::Year,
        breakdown,
    }
}

#[test]
fn validate_accepts_two_distinct_measures() {
    let s = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBreakdown::None,
    );
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn validate_accepts_any_number_of_measures_that_share_the_available_scales() {
    // Three measures, two units — books on one axis, two page measures on the
    // other. The cap is on scales, not on how many measures use them.
    let s = spec(
        vec![
            ChartMeasure::BooksFinished,
            ChartMeasure::AvgPageLength,
            ChartMeasure::PagesRead,
        ],
        ChartBreakdown::None,
    );
    assert_eq!(s.validate(), Ok(()));

    // The widest chart the vocabulary allows: three minutes measures plus two
    // page ones, still only two scales.
    let wide = spec(
        vec![
            ChartMeasure::ReadingMinutes,
            ChartMeasure::ListeningMinutes,
            ChartMeasure::AvgSessionMinutes,
            ChartMeasure::AvgPageLength,
            ChartMeasure::PagesRead,
        ],
        ChartBreakdown::None,
    );
    assert_eq!(wide.validate(), Ok(()));
    assert_eq!(wide.units().len(), MAX_AXES);
}

#[test]
fn validate_rejects_a_measure_needing_a_third_scale() {
    let s = spec(
        vec![
            ChartMeasure::BooksFinished,
            ChartMeasure::AvgPageLength,
            ChartMeasure::AvgRating,
        ],
        ChartBreakdown::None,
    );
    assert_eq!(
        s.validate(),
        Err(ChartSpecError::TooManyUnits("Avg rating"))
    );
}

#[test]
fn validate_rejects_the_same_measure_plotted_twice() {
    let s = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::BooksFinished],
        ChartBreakdown::None,
    );
    assert_eq!(
        s.validate(),
        Err(ChartSpecError::DuplicateMeasures("Books finished"))
    );
}

#[test]
fn units_are_ordered_by_when_they_first_claimed_an_axis() {
    let s = spec(
        vec![
            ChartMeasure::AvgPageLength,
            ChartMeasure::BooksFinished,
            ChartMeasure::PagesRead,
        ],
        ChartBreakdown::None,
    );
    assert_eq!(s.units(), vec![ChartUnit::Pages, ChartUnit::Books]);
}

#[test]
fn axis_for_puts_a_shared_unit_on_the_axis_it_already_claimed() {
    let s = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBreakdown::None,
    );
    // Already on the chart, so it reports the axis it sits on.
    assert_eq!(s.axis_for(ChartMeasure::BooksFinished), Some(0));
    // A second pages measure joins the axis pages already own.
    assert_eq!(s.axis_for(ChartMeasure::PagesRead), Some(1));
    // A third unit has nowhere to go.
    assert_eq!(s.axis_for(ChartMeasure::AvgRating), None);
}

#[test]
fn axis_for_hands_a_new_unit_the_free_axis_while_one_remains() {
    let s = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::None);
    assert_eq!(s.axis_for(ChartMeasure::AvgRating), Some(1));
    assert_eq!(s.axis_for(ChartMeasure::SessionCount), Some(1));
}

#[test]
fn can_add_narrows_the_list_only_once_both_scales_are_claimed() {
    // One measure chosen: everything else is still reachable.
    let one = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::None);
    for m in ChartMeasure::ALL {
        assert_eq!(
            one.can_add(m),
            m != ChartMeasure::BooksFinished,
            "{}",
            m.label()
        );
    }

    // Both scales claimed: only those two units remain.
    let two = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBreakdown::None,
    );
    assert!(two.can_add(ChartMeasure::PagesRead));
    assert!(!two.can_add(ChartMeasure::AvgRating));
    assert!(!two.can_add(ChartMeasure::ReadingMinutes));
    // Already selected, so not addable again.
    assert!(!two.can_add(ChartMeasure::BooksFinished));
}

#[test]
fn toggle_adds_removes_and_preserves_selection_order() {
    let mut s = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::None);
    s.toggle(ChartMeasure::AvgPageLength);
    s.toggle(ChartMeasure::PagesRead);
    assert_eq!(
        s.measures,
        vec![
            ChartMeasure::BooksFinished,
            ChartMeasure::AvgPageLength,
            ChartMeasure::PagesRead
        ]
    );

    // Removing the middle one leaves the rest in order, so the left axis
    // keeps belonging to the measure that claimed it.
    s.toggle(ChartMeasure::AvgPageLength);
    assert_eq!(
        s.measures,
        vec![ChartMeasure::BooksFinished, ChartMeasure::PagesRead]
    );

    // An incompatible measure is a no-op — the control offering it is already
    // disabled, so a throw here would only duplicate that guard.
    s.toggle(ChartMeasure::AvgRating);
    assert_eq!(s.measures.len(), 2);
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn toggling_a_measure_off_frees_its_scale_for_another_unit() {
    let mut s = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBreakdown::None,
    );
    assert!(!s.can_add(ChartMeasure::AvgRating));
    s.toggle(ChartMeasure::AvgPageLength);
    assert!(s.can_add(ChartMeasure::AvgRating));
}

#[test]
fn validate_rejects_a_breakdown_alongside_two_measures() {
    let s = spec(
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength],
        ChartBreakdown::Genre,
    );
    assert_eq!(s.validate(), Err(ChartSpecError::BreakdownNeedsOneMeasure));
}

#[test]
fn validate_rejects_a_breakdown_on_a_measure_that_cannot_carry_one() {
    let s = spec(vec![ChartMeasure::ReadingMinutes], ChartBreakdown::Genre);
    assert_eq!(
        s.validate(),
        Err(ChartSpecError::BreakdownUnsupported("Reading minutes"))
    );
}

#[test]
fn validate_accepts_a_breakdown_on_a_single_completion_measure() {
    let s = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::Genre);
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn empty_bucket_is_zero_for_totals_and_absent_for_averages() {
    assert_eq!(ChartAggregate::Count.empty_bucket(), Some(0.0));
    assert_eq!(ChartAggregate::Sum.empty_bucket(), Some(0.0));
    assert_eq!(ChartAggregate::Average.empty_bucket(), None);
}

#[test]
fn mark_is_derived_from_the_aggregate_not_chosen() {
    assert_eq!(ChartMeasure::BooksFinished.mark(), ChartMark::Bar);
    assert_eq!(ChartMeasure::PagesRead.mark(), ChartMark::Bar);
    assert_eq!(ChartMeasure::AvgPageLength.mark(), ChartMark::Line);
    assert_eq!(ChartMeasure::AvgRating.mark(), ChartMark::Line);
}

#[test]
fn only_completion_grain_measures_support_a_breakdown() {
    for m in ChartMeasure::ALL {
        assert_eq!(
            m.supports_breakdown(),
            m.grain() == ChartGrain::Completion,
            "{}",
            m.label()
        );
    }
}

#[test]
fn from_query_round_trips_every_measure() {
    for m in ChartMeasure::ALL {
        assert_eq!(ChartMeasure::from_query(m.as_query()), Some(m));
    }
    assert_eq!(ChartMeasure::from_query("not_a_measure"), None);
}

#[test]
fn caveats_deduplicate_across_measures_that_share_one() {
    // Both carry a caveat, and they differ — so both survive, in order.
    let s = spec(
        vec![ChartMeasure::PagesRead, ChartMeasure::AvgPageLength],
        ChartBreakdown::None,
    );
    let c = s.caveats();
    assert_eq!(c.len(), 2);
    assert!(c[0].starts_with("Pages read is measured"));

    // A measure with no caveat contributes nothing.
    let s = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::None);
    assert!(s.caveats().is_empty());
}

#[test]
fn series_label_qualifies_the_measure_with_its_slice() {
    let mut s = ChartSeries {
        measure: ChartMeasure::BooksFinished,
        slice: None,
        axis: 0,
        mark: ChartMark::Bar,
        values: vec![],
    };
    assert_eq!(s.label(), "Books finished");
    s.slice = Some("Fantasy".into());
    assert_eq!(s.label(), "Books finished · Fantasy");
}

#[test]
fn series_max_ignores_absent_buckets_and_is_none_when_all_absent() {
    let s = ChartSeries {
        measure: ChartMeasure::AvgPageLength,
        slice: None,
        axis: 0,
        mark: ChartMark::Line,
        values: vec![None, Some(320.0), None, Some(180.0)],
    };
    assert_eq!(s.max(), Some(320.0));

    let empty = ChartSeries {
        values: vec![None, None],
        ..s
    };
    assert_eq!(empty.max(), None);
}

#[test]
fn result_is_empty_when_no_bucket_carries_a_value() {
    let all_absent = ChartResult {
        bucket: ChartBucket::Month,
        buckets: vec!["2026-01".into(), "2026-02".into()],
        series: vec![ChartSeries {
            measure: ChartMeasure::AvgRating,
            slice: None,
            axis: 0,
            mark: ChartMark::Line,
            values: vec![None, None],
        }],
        axes: vec![ChartAxis {
            unit: ChartUnit::Stars,
            max: 1.0,
        }],
        divisions: 4,
        stacked: false,
        truncated: false,
        caveats: vec![],
    };
    assert!(all_absent.is_empty());

    let mut has_one = all_absent.clone();
    has_one.series[0].values[1] = Some(4.0);
    assert!(!has_one.is_empty());
}

#[test]
fn result_with_no_buckets_is_empty_even_when_a_series_exists() {
    let r = ChartResult {
        bucket: ChartBucket::Month,
        buckets: vec![],
        series: vec![ChartSeries {
            measure: ChartMeasure::BooksFinished,
            slice: None,
            axis: 0,
            mark: ChartMark::Bar,
            values: vec![],
        }],
        axes: vec![],
        divisions: 4,
        stacked: false,
        truncated: false,
        caveats: vec![],
    };
    assert!(r.is_empty());
}

#[test]
fn the_default_spec_plots_nothing_and_that_is_valid() {
    let d = ChartSpec::default();
    assert!(d.measures.is_empty());
    assert_eq!(d.validate(), Ok(()));
    // Nothing has claimed a scale, so the whole vocabulary is available and
    // the compatibility rule is visible rather than discovered by being
    // blocked.
    for m in ChartMeasure::ALL {
        assert!(d.can_add(m), "{} unavailable on an empty chart", m.label());
    }
}

#[test]
fn an_emptied_selection_is_valid_rather_than_an_error() {
    let mut s = spec(vec![ChartMeasure::BooksFinished], ChartBreakdown::None);
    s.toggle(ChartMeasure::BooksFinished);
    assert!(s.measures.is_empty());
    assert_eq!(s.validate(), Ok(()));
}

#[test]
fn every_measure_round_trips_through_serde() {
    for m in ChartMeasure::ALL {
        let json = serde_json::to_string(&m).expect("serialize");
        let back: ChartMeasure = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, m);
        // The serde rename and the query name are the same string, so a spec
        // in a URL and a spec on the wire cannot drift apart.
        assert_eq!(json, format!("\"{}\"", m.as_query()));
    }
}

#[test]
fn every_measure_carries_a_sentence_saying_what_it_counts() {
    for m in ChartMeasure::ALL {
        let d = m.description();
        assert!(!d.is_empty(), "{} has no description", m.label());
        assert!(d.ends_with('.'), "{}: {d}", m.label());
        // The label names it; the description has to say something the label
        // does not, or the notes are just repeating the picker.
        assert_ne!(d.trim_end_matches('.'), m.label());
    }
}

// ── Axis fitting ────────────────────────────────────────────────────────

#[test]
fn nice_step_rounds_up_to_a_readable_tick() {
    assert_eq!(nice_step(0.9), 1.0);
    assert_eq!(nice_step(1.2), 2.0);
    assert_eq!(nice_step(2.4), 2.5);
    assert_eq!(nice_step(137.5), 200.0);
}

#[test]
fn nice_axis_keeps_the_data_filling_most_of_the_frame() {
    // The case the old round-the-maximum ladder got wrong: a peak of 550
    // took a 1000 axis and drew every shape at half height.
    let (max, divisions) = nice_axis(550.0, false);
    assert_eq!((max, divisions), (600.0, 3));
    assert!(550.0 / max > 0.9);

    // Stars: a 4.5 peak must not open an axis past the 5 they top out at.
    assert_eq!(nice_axis(4.5, false), (5.0, 5));
}

#[test]
fn nice_axis_gives_a_small_count_one_gridline_per_whole_unit() {
    // Half a book is not a quantity an axis should offer to measure.
    assert_eq!(nice_axis(6.0, true), (6.0, 6));
    assert_eq!(nice_axis(2.0, true), (2.0, 2));
    assert_eq!(nice_axis(1.0, true), (1.0, 1));
    // Without the integral flag the same peak takes the ladder's answer.
    assert_eq!(nice_axis(2.0, false), (2.0, 4));
}

#[test]
fn nice_axis_keeps_a_large_count_on_whole_number_steps() {
    let (max, divisions) = nice_axis(38.0, true);
    assert_eq!((max, divisions), (40.0, 4));
    // Every gridline lands on an integer.
    assert_eq!(max % divisions as f64, 0.0);
}

#[test]
fn nice_axis_never_returns_a_max_below_the_data() {
    for peak in [0.3, 1.0, 6.0, 47.0, 317.0, 550.0, 999.0, 1_000_000.0] {
        for integral in [false, true] {
            let (max, _) = nice_axis(peak, integral);
            assert!(max >= peak, "axis {max} is under its peak {peak}");
        }
    }
}

#[test]
fn nice_axis_never_returns_zero_so_the_renderer_cannot_divide_by_it() {
    assert_eq!(nice_axis(0.0, false).0, 1.0);
    assert_eq!(nice_axis(-5.0, false).0, 1.0);
    assert_eq!(nice_axis(f64::NAN, false).0, 1.0);
    assert_eq!(nice_axis(0.0, true).0, 1.0);
    assert_eq!(axis_max_on(0.0, 4, false), 1.0);
}

#[test]
fn both_axes_share_one_gridline_count() {
    // The right axis is fitted to the left's tick count, so a label on the
    // right always sits on a line that is actually drawn.
    let divisions = 5;
    let right = axis_max_on(450.0, divisions, false);
    assert_eq!(right, 500.0);
    assert!((right / divisions as f64 - 100.0).abs() < f64::EPSILON);
}

#[test]
fn fit_axes_scales_a_stacked_axis_to_the_tallest_column() {
    let slice = |name: &str, values: Vec<Option<f64>>| ChartSeries {
        measure: ChartMeasure::BooksFinished,
        slice: Some(name.to_string()),
        axis: 0,
        mark: ChartMark::Bar,
        values,
    };
    let series = vec![
        slice("Fantasy", vec![Some(2.0), Some(1.0)]),
        slice("Horror", vec![Some(2.0), Some(1.0)]),
        slice("Crime", vec![Some(2.0), Some(1.0)]),
    ];

    // Stacked: the first column is 6, so the axis has to clear 6.
    let (axes, _) = fit_axes(&series, true, 2);
    assert!(axes[0].max >= 6.0, "{:?}", axes[0]);

    // Grouped: no column, so the tallest single slice is 2.
    let (axes, _) = fit_axes(&series, false, 2);
    assert_eq!(axes[0].max, 2.0);
}

#[test]
fn fit_axes_gives_a_second_unit_its_own_axis_on_the_shared_tick_count() {
    let series = vec![
        ChartSeries {
            measure: ChartMeasure::BooksFinished,
            slice: None,
            axis: 0,
            mark: ChartMark::Bar,
            values: vec![Some(4.0)],
        },
        ChartSeries {
            measure: ChartMeasure::AvgPageLength,
            slice: None,
            axis: 1,
            mark: ChartMark::Line,
            values: vec![Some(450.0)],
        },
    ];
    let (axes, divisions) = fit_axes(&series, false, 1);
    assert_eq!(axes.len(), 2);
    assert_eq!(axes[0].unit, ChartUnit::Books);
    assert_eq!(axes[1].unit, ChartUnit::Pages);
    // Both axes divide into the same number of gridlines, or the right-hand
    // labels would sit between the lines they belong to.
    assert!(axes[1].max >= 450.0);
    assert!(divisions >= 3);
}
