//! Tests for the library-size card's formatting and its refusal to render a
//! figure nothing has been measured for.

use super::*;

fn measured(total: i64, books: i64) -> MeasuredTotal {
    MeasuredTotal { total, books }
}

#[test]
fn compact_scales_into_k_m_and_b_and_leaves_small_counts_alone() {
    assert_eq!(compact(0), "0");
    assert_eq!(compact(812), "812");
    // Under 10,000 stays exact — "9K" loses more than it saves.
    assert_eq!(compact(9_999), "9999");
    assert_eq!(compact(94_200), "94.2K");
    assert_eq!(compact(412_000_000), "412M");
    assert_eq!(compact(1_600_000), "1.6M");
    assert_eq!(compact(2_400_000_000), "2.4B");
}

#[test]
fn audio_value_switches_from_hours_to_days_past_a_week() {
    assert_eq!(audio_value(3600), ("1".into(), "hour"));
    assert_eq!(audio_value(12 * 3600), ("12".into(), "hours"));
    // 94 days is the sentence this card exists to let a reader say; 2,256
    // hours is the same fact nobody can picture.
    assert_eq!(audio_value(94 * 24 * 3600), ("94".into(), "days"));
}

#[test]
fn coverage_always_states_the_denominator() {
    assert_eq!(
        coverage(&measured(1, 1_204), 1_510),
        "across 1,204 of 1,510 books"
    );
}

#[test]
fn build_figures_skips_anything_the_library_has_not_measured() {
    // Words backfilled, no comics or print counts beyond them, no audiobooks
    // probed yet: two figures, not three zeroes.
    let size = LibrarySize {
        books: 1_510,
        words: measured(412_000_000, 1_204),
        pages: measured(1_600_000, 1_204),
        listening_seconds: measured(0, 0),
    };

    let figures = build_figures(&size);

    assert_eq!(figures.len(), 2);
    assert_eq!(figures[0].value, "412M");
    assert_eq!(figures[0].unit, "words");
    assert_eq!(figures[0].coverage, "across 1,204 of 1,510 books");
    assert_eq!(figures[1].unit, "est. pages");
}

#[test]
fn build_figures_is_empty_for_a_library_measured_for_nothing() {
    let size = LibrarySize {
        books: 40,
        ..Default::default()
    };

    assert!(build_figures(&size).is_empty());
}

#[cfg(feature = "server")]
#[test]
fn library_size_card_renders_each_figure_with_its_coverage() {
    let size = LibrarySize {
        books: 1_510,
        words: measured(412_000_000, 1_204),
        pages: measured(1_600_000, 1_204),
        listening_seconds: measured(94 * 24 * 3600, 88),
    };

    let html = crate::test_support::render(rsx! { LibrarySizeCard { size: Some(size) } });

    assert!(html.contains("stats-library-size"), "{html}");
    assert!(html.contains("412M"), "{html}");
    assert!(html.contains("94"), "{html}");
    // Never a bare total: the coverage is what makes the number a fact.
    assert!(html.contains("across 1,204 of 1,510 books"), "{html}");
    assert!(html.contains("across 88 of 1,510 books"), "{html}");
}

#[cfg(feature = "server")]
#[test]
fn library_size_card_renders_nothing_before_the_fetch_lands_or_with_no_measurements() {
    let pending = crate::test_support::render(rsx! { LibrarySizeCard { size: None } });
    assert!(!pending.contains("stats-library-size"), "{pending}");

    let unmeasured = crate::test_support::render(rsx! {
        LibrarySizeCard { size: Some(LibrarySize { books: 40, ..Default::default() }) }
    });
    assert!(!unmeasured.contains("stats-library-size"), "{unmeasured}");
}
