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

#[test]
fn to_iso8601_matches_a_reference_calendar_across_two_centuries() {
    // Cross-checked against Python's `datetime` (the proleptic Gregorian
    // reference) over 1900..2100, including leap days, century non-leap
    // years, and pre-epoch instants — the cases a hand-rolled civil-date
    // conversion gets wrong.
    const CASES: &[(i64, &str)] = &[
        (-2208988800, "1900-01-01T00:00:00Z"),
        (-1816961494, "1912-06-04T08:28:26Z"),
        (-1299743367, "1928-10-24T16:10:33Z"),
        (-908313618, "1941-03-21T02:39:42Z"),
        (-849103565, "1943-02-04T09:53:55Z"),
        (-212960532, "1963-04-03T04:17:48Z"),
        (-1, "1969-12-31T23:59:59Z"),
        (0, "1970-01-01T00:00:00Z"),
        (62235419, "1971-12-22T07:36:59Z"),
        (68169600, "1972-02-29T00:00:00Z"),
        (951782400, "2000-02-29T00:00:00Z"),
        (1071627855, "2003-12-17T02:24:15Z"),
        (1075846907, "2004-02-03T22:21:47Z"),
        (1241899182, "2009-05-09T19:59:42Z"),
        (1583020800, "2020-03-01T00:00:00Z"),
        (1779382644, "2026-05-21T16:57:24Z"),
        (1881488336, "2029-08-15T11:38:56Z"),
        (1952001797, "2031-11-09T14:43:17Z"),
        (1953386333, "2031-11-25T15:18:53Z"),
        (2248253200, "2041-03-30T10:46:40Z"),
        (2298135866, "2042-10-28T19:04:26Z"),
        (2524608000, "2050-01-01T00:00:00Z"),
        (2643079206, "2053-10-03T04:40:06Z"),
        (2660494025, "2054-04-22T18:07:05Z"),
        (2733128586, "2056-08-10T10:23:06Z"),
        (2878924233, "2061-03-24T21:10:33Z"),
        (3052914670, "2066-09-28T15:51:10Z"),
        (3259558916, "2073-04-16T09:01:56Z"),
        (3456697643, "2079-07-16T01:47:23Z"),
        (3660708293, "2086-01-01T07:24:53Z"),
        (4084434178, "2099-06-06T13:02:58Z"),
        (4102444800, "2100-01-01T00:00:00Z"),
    ];
    for (epoch, expected) in CASES {
        assert_eq!(&to_iso8601(*epoch), expected, "epoch {epoch}");
    }
}

#[test]
fn to_iso8601_renders_both_ends_of_the_i64_range_without_overflowing() {
    // The module claims validity across the whole range; a caller that
    // believed it and got a panic would take the process down over a
    // corrupt timestamp. Extended years are absurd but well-formed, which
    // is the point — every epoch renders, so no caller needs a fallback.
    assert_eq!(to_iso8601(i64::MAX), "292277026596-12-04T15:30:07Z");
    assert_eq!(to_iso8601(i64::MIN), "-292277022657-01-27T08:29:52Z");
}
