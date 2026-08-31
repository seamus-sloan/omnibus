//! Tests for the notes panel. Every sentence it writes is derived from the
//! result, so these assert the derivation rather than the prose.

use omnibus_shared::{ChartAxis, ChartBucket, ChartMeasure, ChartSeries, ChartUnit};

use super::*;
use crate::test_support::render_in_vdom;

fn series(measure: ChartMeasure, axis: u8, slice: Option<&str>) -> ChartSeries {
    ChartSeries {
        measure,
        slice: slice.map(str::to_string),
        axis,
        mark: measure.mark(),
        values: vec![Some(1.0), Some(2.0)],
    }
}

fn result(series: Vec<ChartSeries>, axes: Vec<ChartAxis>) -> ChartResult {
    ChartResult {
        bucket: ChartBucket::Month,
        buckets: vec!["2026-01".into(), "2026-02".into()],
        series,
        axes,
        divisions: 4,
        stacked: false,
        truncated: false,
        caveats: vec![],
    }
}

fn axis(unit: ChartUnit) -> ChartAxis {
    ChartAxis { unit, max: 10.0 }
}

/// The default chart: a count against an average, on two scales.
fn two_scales() -> ChartResult {
    result(
        vec![
            series(ChartMeasure::BooksFinished, 0, None),
            series(ChartMeasure::AvgPageLength, 1, None),
        ],
        vec![axis(ChartUnit::Books), axis(ChartUnit::Pages)],
    )
}

fn one_scale() -> ChartResult {
    result(
        vec![
            series(ChartMeasure::ReadingMinutes, 0, None),
            series(ChartMeasure::ListeningMinutes, 0, None),
        ],
        vec![axis(ChartUnit::Minutes)],
    )
}

// ── Sentence derivation ─────────────────────────────────────────────────

#[test]
fn join_units_reads_as_a_list_a_person_would_write() {
    assert_eq!(join_units(&[]), "");
    assert_eq!(join_units(&[ChartUnit::Books]), "books");
    assert_eq!(
        join_units(&[ChartUnit::Books, ChartUnit::Pages]),
        "books and pages"
    );
    assert_eq!(
        join_units(&[ChartUnit::Books, ChartUnit::Pages, ChartUnit::Minutes]),
        "books, pages and minutes"
    );
}

#[test]
fn the_scales_note_warns_that_a_crossing_means_nothing_on_two_axes() {
    let note = scales_note(&two_scales()).expect("a scales note");
    assert!(note.contains("books on the left"), "{note}");
    assert!(note.contains("pages on the right"), "{note}");
    // The single easiest thing to misread on this chart, said outright.
    assert!(note.contains("means nothing"), "{note}");
}

#[test]
fn the_scales_note_says_one_scale_is_directly_comparable() {
    let note = scales_note(&one_scale()).expect("a scales note");
    assert!(note.contains("minutes"), "{note}");
    assert!(note.contains("directly comparable"), "{note}");
    // No crossing warning, because there is no second scale to misread.
    assert!(!note.contains("means nothing"), "{note}");
}

#[test]
fn the_scales_note_is_absent_when_there_is_nothing_to_scale() {
    assert_eq!(scales_note(&result(vec![], vec![])), None);
}

#[test]
fn the_availability_note_names_the_units_holding_the_scales() {
    let note = availability_note(&two_scales());
    assert!(note.contains("books and pages"), "{note}");
    assert!(note.contains("greyed out"), "{note}");
    // And says what would free one, rather than leaving it as a dead end.
    assert!(note.contains("free a scale"), "{note}");
}

#[test]
fn the_availability_note_says_so_while_a_scale_is_free() {
    let note = availability_note(&one_scale());
    assert!(note.contains("still free"), "{note}");
    assert!(!note.contains("greyed out"), "{note}");
}

#[test]
fn the_split_note_explains_stacking_only_when_the_split_stacks() {
    let mut stacked = result(
        vec![
            series(ChartMeasure::BooksFinished, 0, Some("Fantasy")),
            series(ChartMeasure::BooksFinished, 0, Some("Horror")),
        ],
        vec![axis(ChartUnit::Books)],
    );
    stacked.stacked = true;
    let note = split_note(&stacked).expect("a split note");
    assert!(note.contains("slices stack"), "{note}");

    // An average split cannot stack, and the note says why rather than
    // leaving the difference unexplained.
    let grouped = result(
        vec![
            series(ChartMeasure::AvgPageLength, 0, Some("Fantasy")),
            series(ChartMeasure::AvgPageLength, 0, Some("Horror")),
        ],
        vec![axis(ChartUnit::Pages)],
    );
    let note = split_note(&grouped).expect("a split note");
    assert!(note.contains("averages don't add up"), "{note}");
}

#[test]
fn the_split_note_mentions_the_fold_only_when_something_was_folded() {
    let mut folded = result(
        vec![
            series(ChartMeasure::BooksFinished, 0, Some("Fantasy")),
            series(ChartMeasure::BooksFinished, 0, Some("Other")),
        ],
        vec![axis(ChartUnit::Books)],
    );
    folded.stacked = true;
    assert!(split_note(&folded).unwrap().contains("folded into Other"));

    let mut whole = result(
        vec![series(ChartMeasure::BooksFinished, 0, Some("Fantasy"))],
        vec![axis(ChartUnit::Books)],
    );
    whole.stacked = true;
    assert!(!split_note(&whole).unwrap().contains("folded into Other"));
}

#[test]
fn there_is_no_split_note_without_a_split() {
    assert_eq!(split_note(&two_scales()), None);
}

#[test]
fn the_empty_note_distinguishes_a_zero_total_from_an_absent_average() {
    // Both kinds on screen: the sentence has to cover both.
    let note = empty_note(&two_scales()).expect("an empty note");
    assert!(note.contains("zero for the totals"), "{note}");
    assert!(note.contains("can't average nothing"), "{note}");

    // Averages only.
    let avg = result(
        vec![series(ChartMeasure::AvgRating, 0, None)],
        vec![axis(ChartUnit::Stars)],
    );
    let note = empty_note(&avg).expect("an empty note");
    assert!(note.contains("line breaks"), "{note}");

    // Totals only: a zero is just a zero, so there is nothing to explain.
    let totals = result(
        vec![series(ChartMeasure::BooksFinished, 0, None)],
        vec![axis(ChartUnit::Books)],
    );
    assert_eq!(empty_note(&totals), None);
}

// ── Rendered panel ──────────────────────────────────────────────────────

#[test]
fn the_panel_describes_every_measure_once_with_its_grain_and_unit() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: two_scales() }
        }
    });
    assert!(html.contains("Books you marked finished"), "{html}");
    assert!(
        html.contains("How long those finished books were"),
        "{html}"
    );
    assert!(
        html.contains("Measured per book finished, in books."),
        "{html}"
    );
    assert!(
        html.contains("Measured per book finished, in pages."),
        "{html}"
    );
}

/// One measure cut into three genre slices.
fn split_three_ways() -> ChartResult {
    ChartResult {
        stacked: true,
        ..result(
            vec![
                series(ChartMeasure::BooksFinished, 0, Some("Fantasy")),
                series(ChartMeasure::BooksFinished, 0, Some("Horror")),
                series(ChartMeasure::BooksFinished, 0, Some("Crime")),
            ],
            vec![axis(ChartUnit::Books)],
        )
    }
}

fn with_caveat() -> ChartResult {
    ChartResult {
        caveats: vec!["Book length is estimated.".into()],
        ..two_scales()
    }
}

fn clipped() -> ChartResult {
    ChartResult {
        truncated: true,
        ..two_scales()
    }
}

#[test]
fn the_panel_names_a_split_measure_once_rather_than_once_per_slice() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: split_three_ways() }
        }
    });
    assert_eq!(
        html.matches("Books you marked finished").count(),
        1,
        "{html}"
    );
}

#[test]
fn the_panel_folds_the_caveats_in_under_their_own_heading() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: with_caveat() }
        }
    });
    assert!(html.contains("What these numbers can"), "{html}");
    assert!(html.contains("chart-caveat"), "{html}");
    assert!(html.contains("Book length is estimated."), "{html}");
}

#[test]
fn the_panel_omits_the_caveat_heading_when_there_are_none() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: two_scales() }
        }
    });
    assert!(
        !html.contains("What these numbers can't tell you"),
        "{html}"
    );
}

#[test]
fn the_panel_reports_a_clipped_axis() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: clipped() }
        }
    });
    assert!(html.contains("chart-truncated"), "{html}");
    assert!(html.contains("most recent"), "{html}");
}

#[test]
fn the_panel_renders_nothing_when_there_is_no_chart_to_describe() {
    let html = render_in_vdom(|| {
        rsx! {
            ChartNotes { result: result(vec![], vec![]) }
        }
    });
    assert!(!html.contains("chart-notes"), "{html}");
}
