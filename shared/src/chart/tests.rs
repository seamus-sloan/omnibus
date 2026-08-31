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
fn validate_rejects_a_spec_with_no_measures() {
    assert_eq!(
        spec(vec![], ChartBreakdown::None).validate(),
        Err(ChartSpecError::NoMeasures)
    );
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
        truncated: false,
        caveats: vec![],
    };
    assert!(r.is_empty());
}

#[test]
fn the_default_spec_is_the_comparison_the_builder_exists_for() {
    let d = ChartSpec::default();
    assert_eq!(d.validate(), Ok(()));
    assert_eq!(
        d.measures,
        vec![ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength]
    );
    // Two grains, one shared bucket — the case a single pivot query cannot serve.
    assert_eq!(d.measures[0].grain(), ChartGrain::Completion);
    assert_eq!(d.measures[1].grain(), ChartGrain::Completion);
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
