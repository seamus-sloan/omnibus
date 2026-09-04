use super::*;

#[test]
fn to_iso8601_renders_the_epoch_itself() {
    assert_eq!(to_iso8601(0), "1970-01-01T00:00:00Z");
}

#[test]
fn to_iso8601_matches_known_timestamps() {
    assert_eq!(to_iso8601(1_000_000_000), "2001-09-09T01:46:40Z");
    assert_eq!(to_iso8601(1_700_000_000), "2023-11-14T22:13:20Z");
    // A leap day, which the month arithmetic has to place correctly.
    assert_eq!(to_iso8601(1_709_164_800), "2024-02-29T00:00:00Z");
    // The last second of a leap year.
    assert_eq!(to_iso8601(1_735_689_599), "2024-12-31T23:59:59Z");
    // A century that is not a leap year (1900 was not; 2000 was).
    assert_eq!(to_iso8601(951_782_400), "2000-02-29T00:00:00Z");
}

#[test]
fn to_iso8601_borrows_a_day_for_a_pre_epoch_timestamp() {
    // Truncating toward zero would report 1970-01-01T00:00:00Z here; floor
    // division is what puts it on the previous day.
    assert_eq!(to_iso8601(-1), "1969-12-31T23:59:59Z");
    assert_eq!(to_iso8601(-86_400), "1969-12-31T00:00:00Z");
}

#[test]
fn to_iso8601_output_is_fixed_width_and_sorts_chronologically() {
    let earlier = to_iso8601(1_700_000_000);
    let later = to_iso8601(1_800_000_000);
    assert_eq!(earlier.len(), later.len());
    assert!(earlier < later, "ISO output must sort as it reads");
}

#[test]
fn to_iso8601_opt_passes_absence_through() {
    assert_eq!(to_iso8601_opt(None), None);
    assert_eq!(
        to_iso8601_opt(Some(0)).as_deref(),
        Some("1970-01-01T00:00:00Z")
    );
}
