//! Tests for the builder page: the picker's rendered contract, and the
//! selection invariants the controls enforce before a spec can reach the wire.

use super::*;
use crate::test_support::render_in_vdom;

/// A checkbox row's rendered `<input …>` tag, so a test can assert on its
/// attributes without matching the whole document.
fn input_tag(html: &str, measure: ChartMeasure) -> String {
    let id = format!(r#"id="cb-m-{}""#, measure.as_query());
    let at = html
        .find(&id)
        .unwrap_or_else(|| panic!("no row for {id}: {html}"));
    let start = html[..at].rfind("<input").expect("an input opening tag");
    let end = at + html[at..].find('>').expect("a closing bracket");
    html[start..=end].to_string()
}

#[test]
fn the_measure_group_offers_every_measure_with_its_unit_and_grain() {
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
        assert!(
            html.contains(&format!(r#"id="cb-m-{}""#, m.as_query())),
            "missing checkbox for {}",
            m.label()
        );
    }
    // The unit is what decides compatibility, so the row states it.
    assert!(html.contains("books · per book finished"));
    assert!(html.contains("minutes · per sitting"));
}

#[test]
fn a_measure_is_offered_while_a_scale_is_free_and_greyed_once_both_are_claimed() {
    // One measure: one scale free, so nothing is blocked.
    let one = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::BooksFinished],
                    ..ChartSpec::default()
                })
            }
        }
    });
    assert!(!input_tag(&one, ChartMeasure::AvgRating).contains("disabled"));
    assert!(!input_tag(&one, ChartMeasure::ReadingMinutes).contains("disabled"));

    // Books + pages claims both scales. A third unit is greyed out; a second
    // pages measure is not, because it joins a scale that already exists.
    let two = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    assert!(input_tag(&two, ChartMeasure::AvgRating).contains("disabled"));
    assert!(input_tag(&two, ChartMeasure::ReadingMinutes).contains("disabled"));
    assert!(!input_tag(&two, ChartMeasure::PagesRead).contains("disabled"));
}

#[test]
fn the_last_remaining_measure_cannot_be_unchecked() {
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
    // An empty chart is not a state the picker should be able to reach.
    let tag = input_tag(&html, ChartMeasure::BooksFinished);
    assert!(tag.contains("checked"), "{tag}");
    assert!(tag.contains("disabled"), "{tag}");
}

#[test]
fn a_selected_measure_stays_uncheckable_off_only_while_others_remain() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    // Two selected, so either may be removed.
    for m in [ChartMeasure::BooksFinished, ChartMeasure::AvgPageLength] {
        let tag = input_tag(&html, m);
        assert!(tag.contains("checked"), "{m:?}: {tag}");
        assert!(!tag.contains("disabled"), "{m:?}: {tag}");
    }
}

#[test]
fn the_scales_hint_says_whether_another_unit_can_still_join() {
    let free = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::BooksFinished],
                    ..ChartSpec::default()
                })
            }
        }
    });
    assert!(free.contains("anything else can still join"));

    let full = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    assert!(full.contains("Both scales in use"));
}

#[test]
fn the_split_control_is_disabled_while_more_than_one_measure_is_selected() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    assert!(html.contains("one measure only"));
}

#[test]
fn the_bucket_and_period_selects_mark_the_spec_s_current_choice() {
    // A `value` on <select> does not pick an option in server-rendered
    // markup; `selected` on the option does.
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls {
                spec: Signal::new(ChartSpec {
                    measures: vec![ChartMeasure::PagesRead],
                    bucket: ChartBucket::Year,
                    range: omnibus_shared::StatsRange::AllTime,
                    breakdown: ChartBreakdown::None,
                })
            }
        }
    });
    assert!(html.contains(r#"<option value="year" selected"#), "{html}");
    assert!(
        html.contains(r#"<option value="all_time" selected"#),
        "{html}"
    );
}

#[test]
fn the_page_renders_every_picker_control() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartControls { spec: Signal::new(ChartSpec::default()) }
        }
    });
    for id in [
        "cb-m-books_finished",
        "cb-bucket",
        "cb-range",
        "cb-breakdown",
    ] {
        assert!(html.contains(id), "missing control {id}: {html}");
    }
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
                    stacked: false,
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
                    buckets: vec!["2026-01".into()],
                    // The notes describe the chart, so they need one to
                    // describe — an empty result has nothing to say.
                    series: vec![omnibus_shared::ChartSeries {
                        measure: ChartMeasure::PagesRead,
                        slice: None,
                        axis: 0,
                        mark: omnibus_shared::ChartMark::Bar,
                        values: vec![Some(2.0)],
                    }],
                    axes: vec![omnibus_shared::ChartAxis {
                        unit: omnibus_shared::ChartUnit::Pages,
                        max: 5.0,
                    }],
                    divisions: 4,
                    stacked: false,
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
                    stacked: false,
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
