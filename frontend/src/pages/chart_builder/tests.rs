//! Tests for the builder page: the picker's rendered contract, and the
//! selection invariants the controls enforce before a spec can reach the wire.

use super::*;
use crate::test_support::render_in_vdom;

#[test]
fn the_page_renders_every_picker_control() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    for id in [
        "cb-measure-a",
        "cb-measure-b",
        "cb-bucket",
        "cb-range",
        "cb-breakdown",
    ] {
        assert!(html.contains(id), "missing control {id}: {html}");
    }
}

#[test]
fn the_measure_picker_offers_every_measure_in_the_vocabulary() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    for m in ChartMeasure::ALL {
        assert!(
            html.contains(m.label()),
            "missing measure {}: {html}",
            m.label()
        );
    }
}

#[test]
fn the_picker_names_the_grain_each_selected_measure_is_measured_at() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    // The default spec is two completion-grain measures, so the hint that
    // explains *why* they can share a bucket is on screen.
    assert!(
        html.contains("measured per book finished"),
        "expected a grain hint: {html}"
    );
}

#[test]
fn the_split_control_is_disabled_while_a_comparison_is_selected() {
    // The default spec carries two measures, which leaves no axis for a split.
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    assert!(
        html.contains("disabled"),
        "expected a disabled split: {html}"
    );
    assert!(html.contains("drop the comparison to split"));
}

#[test]
fn the_split_control_opens_for_a_single_per_book_measure() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::BooksFinished],
                    ..ChartSpec::default()
                })
            }
        }
    });
    assert!(html.contains("top genres, rest folded"));
}

#[test]
fn the_split_control_explains_itself_on_a_measure_that_cannot_split() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::ReadingMinutes],
                    ..ChartSpec::default()
                })
            }
        }
    });
    assert!(html.contains("only per-book measures split"));
}

#[test]
fn the_comparison_picker_excludes_the_measure_already_on_the_primary_axis() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::BooksFinished],
                    ..ChartSpec::default()
                })
            }
        }
    });
    // One option carries it — the primary select's. The comparison select
    // omits it: the same measure on both axes is not a comparison.
    assert_eq!(
        html.matches(r#"<option value="books_finished""#).count(),
        1,
        "the primary must not be offered as its own comparison: {html}"
    );
}

#[test]
fn an_empty_measure_list_still_names_a_measure_rather_than_unwrapping() {
    // `validate` rejects this before the wire and the picker cannot produce
    // it, but the render path must still be total.
    assert_eq!(
        None.unwrap_or_default_measure(),
        ChartMeasure::BooksFinished
    );
    assert_eq!(
        Some(ChartMeasure::PagesRead).unwrap_or_default_measure(),
        ChartMeasure::PagesRead
    );
}

#[test]
fn the_canvas_renders_the_empty_state_for_a_result_with_no_data() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartCanvas {
                result: Signal::new(Some(ChartResult {
                    bucket: ChartBucket::Month,
                    buckets: vec![],
                    series: vec![],
                    axes: vec![],
                    divisions: 4,
                    truncated: false,
                    caveats: vec![],
                })),
                loading: Signal::new(false),
                error: Signal::new(None),
            }
        }
    });
    assert!(
        html.contains("chart-empty"),
        "expected an empty state: {html}"
    );
    assert!(html.contains("Nothing recorded in this period yet"));
}

#[test]
fn the_canvas_surfaces_a_measures_caveat_alongside_the_chart() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartCanvas {
                result: Signal::new(Some(ChartResult {
                    bucket: ChartBucket::Month,
                    buckets: vec![],
                    series: vec![],
                    axes: vec![],
                    divisions: 4,
                    truncated: true,
                    caveats: vec!["Pages read is measured from the ledger.".into()],
                })),
                loading: Signal::new(false),
                error: Signal::new(None),
            }
        }
    });
    assert!(html.contains("chart-caveat"));
    assert!(html.contains("Pages read is measured from the ledger."));
    // A clipped axis says so rather than passing itself off as the whole range.
    assert!(html.contains("chart-truncated"));
}

#[test]
fn the_canvas_keeps_the_previous_chart_on_screen_while_a_refetch_is_in_flight() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartCanvas {
                result: Signal::new(Some(ChartResult {
                    bucket: ChartBucket::Month,
                    buckets: vec!["2026-01".into()],
                    series: vec![omnibus_shared::ChartSeries {
                        measure: ChartMeasure::BooksFinished,
                        slice: None,
                        axis: 0,
                        mark: omnibus_shared::ChartMark::Bar,
                        values: vec![Some(2.0)],
                    }],
                    axes: vec![omnibus_shared::ChartAxis {
                        unit: omnibus_shared::ChartUnit::Books,
                        max: 5.0,
                    }],
                    divisions: 4,
                    truncated: false,
                    caveats: vec![],
                })),
                loading: Signal::new(true),
                error: Signal::new(None),
            }
        }
    });
    // Still drawn, just marked — a spec change must not flash the surface.
    assert!(
        html.contains("is-loading"),
        "expected a loading marker: {html}"
    );
    assert!(
        html.contains("<rect"),
        "expected the previous chart: {html}"
    );
}

#[test]
fn each_select_marks_the_spec_s_current_choice_as_selected() {
    // A `value` on <select> does not pick an option in server-rendered
    // markup, so without `selected` on the option every control renders
    // showing its first entry regardless of the spec behind it.
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::PagesRead, ChartMeasure::AvgRating],
                    bucket: ChartBucket::Year,
                    range: omnibus_shared::StatsRange::AllTime,
                    breakdown: ChartBreakdown::None,
                })
            }
        }
    });
    for expected in [
        r#"<option value="pages_read" selected"#,
        r#"<option value="avg_rating" selected"#,
        r#"<option value="year" selected"#,
        r#"<option value="all_time" selected"#,
    ] {
        assert!(html.contains(expected), "missing {expected}: {html}");
    }
}

#[test]
fn the_comparison_select_marks_nothing_when_only_one_measure_is_chosen() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::BooksFinished],
                    ..ChartSpec::default()
                })
            }
        }
    });
    assert!(
        html.contains(r#"<option value="__none" selected"#),
        "{html}"
    );
}
