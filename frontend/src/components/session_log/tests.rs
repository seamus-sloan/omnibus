//! Unit tests for the session-log row formatters: the UTC start stamp
//! and the sitting-length label the book-detail stop shares.

use super::*;

#[test]
fn fmt_started_renders_a_utc_month_day_year_and_clock() {
    assert_eq!(fmt_started(1_700_000_000), "Nov 14, 2023 \u{b7} 22:13");
    assert_eq!(fmt_started(0), "Jan 1, 1970 \u{b7} 00:00");
}

#[test]
fn fmt_started_pads_the_clock_to_two_digits() {
    // 1970-01-01 09:05 UTC.
    assert_eq!(fmt_started(9 * 3600 + 5 * 60), "Jan 1, 1970 \u{b7} 09:05");
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
