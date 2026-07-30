//! Tests for the Users settings section's date helpers: `fmt_date`
//! formatting known epochs in UTC and staying stable within a civil day,
//! and `civil_from_days` round-tripping the epoch.

use super::*;

#[test]
fn fmt_date_formats_known_epochs_utc() {
    assert_eq!(fmt_date(0), "Jan 1, 1970");
    // 2024-01-01T00:00:00Z
    assert_eq!(fmt_date(1_704_067_200), "Jan 1, 2024");
    // 2026-07-25T00:00:00Z
    assert_eq!(fmt_date(1_784_937_600), "Jul 25, 2026");
}

#[test]
fn fmt_date_is_stable_within_a_day() {
    // Any second within the civil day maps to the same date (SSR/hydration
    // must not disagree because of sub-day drift).
    assert_eq!(fmt_date(1_704_067_200), fmt_date(1_704_067_200 + 86_399));
}

#[test]
fn civil_from_days_round_trips_epoch() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
}
