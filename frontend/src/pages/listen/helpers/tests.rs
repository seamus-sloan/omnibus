use omnibus_shared::ChapterInfo;

use super::{effective_scrub_position, format_hms, range_fill_pct, remaining_at_rate};

fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
    ChapterInfo {
        ordinal,
        title: title.into(),
        start_seconds: start,
        duration_seconds: dur,
    }
}

/// Mirrors the shape of the real per-target `chapter_index_for_elapsed`
/// implementations (`chapter_nav` / `mobile::view`) without depending on
/// either — this module compiles on both cfg configurations.
fn idx_of(chapters: &[ChapterInfo], elapsed: f64) -> usize {
    chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1)
}

#[test]
fn effective_scrub_position_passes_through_when_not_scrubbing() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    let (effective, idx, remaining) =
        effective_scrub_position(&chs, 120.0, 900.0, 0, 780.0, None, idx_of);
    assert!((effective - 120.0).abs() < f64::EPSILON);
    assert_eq!(idx, 0);
    assert!((remaining - 780.0).abs() < f64::EPSILON);
}

#[test]
fn effective_scrub_position_overrides_with_drag_target_when_scrubbing() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    // Parent still reports elapsed=120/chapter 0/remaining=780, but the
    // drag is previewing 500s — every readout should follow the drag.
    let (effective, idx, remaining) =
        effective_scrub_position(&chs, 120.0, 900.0, 0, 780.0, Some(500.0), idx_of);
    assert!((effective - 500.0).abs() < f64::EPSILON);
    assert_eq!(idx, 1);
    assert!((remaining - 400.0).abs() < f64::EPSILON);
}

#[test]
fn effective_scrub_position_recomputes_chapter_at_a_boundary() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    // Dragging to exactly the chapter-2 boundary should flip the index.
    let (_, idx_before, _) =
        effective_scrub_position(&chs, 0.0, 900.0, 0, 900.0, Some(299.9), idx_of);
    let (_, idx_after, _) =
        effective_scrub_position(&chs, 0.0, 900.0, 0, 900.0, Some(300.0), idx_of);
    assert_eq!(idx_before, 0);
    assert_eq!(idx_after, 1);
}

#[test]
fn effective_scrub_position_clamps_a_drag_target_past_the_end() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0)];
    let (effective, _, remaining) =
        effective_scrub_position(&chs, 0.0, 300.0, 0, 300.0, Some(999.0), idx_of);
    assert!((effective - 300.0).abs() < f64::EPSILON);
    assert!((remaining - 0.0).abs() < f64::EPSILON);
}

#[test]
fn effective_scrub_position_clamps_a_negative_drag_target_to_zero() {
    // A stale/out-of-range scrub value (e.g. one that outlives a
    // mid-drag `duration` change) clamps into range on both platforms —
    // this is the mobile-alignment fix noted on `effective_scrub_position`.
    let chs = vec![ch(1, "Intro", 0.0, 300.0)];
    let (effective, idx, remaining) =
        effective_scrub_position(&chs, 120.0, 300.0, 0, 180.0, Some(-50.0), idx_of);
    assert!((effective - 0.0).abs() < f64::EPSILON);
    assert_eq!(idx, 0);
    assert!((remaining - 300.0).abs() < f64::EPSILON);
}

#[test]
fn format_hms_under_one_hour_renders_mm_ss() {
    assert_eq!(format_hms(0.0), "0:00");
    assert_eq!(format_hms(5.0), "0:05");
    assert_eq!(format_hms(65.0), "1:05");
    assert_eq!(format_hms(599.9), "9:59");
}

#[test]
fn format_hms_past_one_hour_renders_h_mm_ss() {
    assert_eq!(format_hms(3600.0), "1:00:00");
    assert_eq!(format_hms(3661.0), "1:01:01");
    assert_eq!(format_hms(13_596.0), "3:46:36");
}

#[test]
fn format_hms_handles_negative_and_non_finite_as_zero() {
    assert_eq!(format_hms(-12.0), "0:00");
    assert_eq!(format_hms(f64::NAN), "0:00");
    assert_eq!(format_hms(f64::INFINITY), "0:00");
}

#[test]
fn remaining_at_rate_divides_by_the_playback_rate() {
    assert!((remaining_at_rate(600.0, 2.0) - 300.0).abs() < f64::EPSILON);
    assert!((remaining_at_rate(600.0, 0.5) - 1200.0).abs() < f64::EPSILON);
    assert!((remaining_at_rate(600.0, 1.0) - 600.0).abs() < f64::EPSILON);
}

#[test]
fn remaining_at_rate_falls_back_unscaled_for_invalid_rates() {
    assert!((remaining_at_rate(600.0, 0.0) - 600.0).abs() < f64::EPSILON);
    assert!((remaining_at_rate(600.0, -1.0) - 600.0).abs() < f64::EPSILON);
    assert!((remaining_at_rate(600.0, f64::NAN) - 600.0).abs() < f64::EPSILON);
    assert!((remaining_at_rate(600.0, f64::INFINITY) - 600.0).abs() < f64::EPSILON);
}

// Issue #2246 (AC1): the chapter list and the transport total are the same
// scaling applied to the same book seconds, so the parts still sum to the
// whole at any speed — a chapter list that stayed at 1x summed to a
// different book than the total beside it.
#[test]
fn remaining_at_rate_keeps_chapter_durations_summing_to_the_total() {
    let chapters = [1800.0, 1500.0, 300.0];
    let duration: f64 = chapters.iter().sum();
    for rate in [0.5, 1.0, 1.5, 2.0, 3.0] {
        let scaled: f64 = chapters.iter().map(|d| remaining_at_rate(*d, rate)).sum();
        assert!(
            (scaled - remaining_at_rate(duration, rate)).abs() < 1e-9,
            "rate {rate}"
        );
    }
}

#[test]
fn only_the_time_left_is_rate_adjusted_now() {
    // Issue #2344: the player's elapsed and total readouts show real book-time
    // — matching the bookmark stamps and the book detail page — and do NOT
    // scale with the rate. Only the "time left" estimate is rate-adjusted.
    let duration = 3600.0;
    let elapsed = 600.0;
    // Elapsed and total are raw book-time, identical whatever the rate.
    assert_eq!(format_hms(elapsed), "10:00");
    assert_eq!(format_hms(duration), "1:00:00");
    // The time-left halves as the speed doubles.
    assert_eq!(
        format_hms(remaining_at_rate(duration - elapsed, 1.0)),
        "50:00"
    );
    assert_eq!(
        format_hms(remaining_at_rate(duration - elapsed, 2.0)),
        "25:00"
    );
}

#[test]
fn range_fill_pct_maps_a_position_onto_its_own_bounds() {
    // The seek bar starts at zero; the speed slider does not, and reading its
    // fill off zero would leave 0.5x looking a sixth played.
    assert!((range_fill_pct(30.0, 0.0, 120.0) - 25.0).abs() < 1e-9);
    assert!((range_fill_pct(0.5, 0.5, 3.0) - 0.0).abs() < 1e-9);
    assert!((range_fill_pct(1.75, 0.5, 3.0) - 50.0).abs() < 1e-9);
    assert!((range_fill_pct(3.0, 0.5, 3.0) - 100.0).abs() < 1e-9);
}

#[test]
fn range_fill_pct_clamps_outside_the_bounds() {
    assert_eq!(range_fill_pct(-10.0, 0.0, 120.0), 0.0);
    assert_eq!(range_fill_pct(500.0, 0.0, 120.0), 100.0);
}

#[test]
fn range_fill_pct_is_zero_when_the_stop_would_be_nan() {
    // A gradient stop of `NaN%` drops the whole declaration, so a book whose
    // duration hasn't resolved renders an empty track rather than no track.
    assert_eq!(range_fill_pct(10.0, 0.0, 0.0), 0.0);
    assert_eq!(range_fill_pct(10.0, 5.0, 1.0), 0.0);
    assert_eq!(range_fill_pct(f64::NAN, 0.0, 120.0), 0.0);
    assert_eq!(range_fill_pct(10.0, 0.0, f64::INFINITY), 0.0);
}
