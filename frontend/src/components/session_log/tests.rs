//! Unit tests for the session-log row formatters: the start stamp and the
//! sitting-length label the book-detail stop shares.

use super::*;

#[test]
fn fmt_started_renders_a_month_day_year_and_clock_at_the_given_offset() {
    // Offset 0 is the SSR / pre-hydration render (rule 07).
    assert_eq!(fmt_started(1_700_000_000, 0), "Nov 14, 2023 \u{b7} 22:13");
    assert_eq!(fmt_started(0, 0), "Jan 1, 1970 \u{b7} 00:00");
}

#[test]
fn fmt_started_pads_the_clock_to_two_digits() {
    // 1970-01-01 09:05 UTC.
    assert_eq!(
        fmt_started(9 * 3600 + 5 * 60, 0),
        "Jan 1, 1970 \u{b7} 09:05"
    );
}

// Regression for #2464: a sitting at 23:36 on the 7th, west of UTC, was
// stamped "Sep 8 · 03:36" beside a spark the server had already bucketed on
// the reader's own day. Date and clock move together or not at all.
#[test]
fn fmt_started_carries_the_date_back_with_the_clock_west_of_utc() {
    // 2023-11-15 03:36 UTC is 22:36 on the 14th at UTC-5.
    let utc_early_hours = 1_700_019_360;
    assert_eq!(fmt_started(utc_early_hours, 0), "Nov 15, 2023 \u{b7} 03:36");
    assert_eq!(
        fmt_started(utc_early_hours, -5 * 3600),
        "Nov 14, 2023 \u{b7} 22:36"
    );
}

#[test]
fn duration_label_scales_minutes_and_hours() {
    assert_eq!(duration_label(0), "0m");
    assert_eq!(duration_label(60), "1m");
    assert_eq!(duration_label(3600), "1h");
    assert_eq!(duration_label(5400), "1h 30m");
}

#[test]
fn duration_label_clamps_negative_input_to_zero() {
    assert_eq!(duration_label(-100), "0m");
}
