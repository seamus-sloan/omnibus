use super::*;

#[test]
fn group_thousands_handles_short_and_negative_inputs() {
    assert_eq!(group_thousands(0), "0");
    assert_eq!(group_thousands(9), "9");
    assert_eq!(group_thousands(999), "999");
    assert_eq!(group_thousands(1000), "1,000");
    assert_eq!(group_thousands(1_234_567), "1,234,567");
    assert_eq!(group_thousands(-42), "-42");
}

#[test]
fn month_name_spells_the_month_the_window_label_reads_as_prose() {
    assert_eq!(month_name(1), "January");
    assert_eq!(month_name(8), "August");
    assert_eq!(month_name(12), "December");
}

#[test]
fn window_label_says_what_each_range_actually_covers() {
    // Every one is "to date": the current window is period-to-date, and a
    // label reading "August 2026" alone claims a whole month has been counted.
    assert_eq!(
        window_label(StatsRange::Month, "2026-08-14"),
        "August 2026 \u{00B7} month to date"
    );
    assert_eq!(
        window_label(StatsRange::Year, "2026-08-14"),
        "2026 \u{00B7} year to date"
    );
    assert_eq!(
        window_label(StatsRange::AllTime, "2026-08-14"),
        "Everything you have tracked"
    );
}

#[test]
fn window_label_names_the_week_by_its_own_monday() {
    // 2026-08-14 is a Friday; the week it belongs to opened on the 10th. The
    // week's own start, so the label holds whether or not the reader happened
    // to read on the Monday.
    assert_eq!(
        window_label(StatsRange::Week, "2026-08-14"),
        "Week of 10 Aug 2026 \u{00B7} to date"
    );
    // A Monday labels itself.
    assert_eq!(
        window_label(StatsRange::Week, "2026-08-10"),
        "Week of 10 Aug 2026 \u{00B7} to date"
    );
}

#[test]
fn window_label_falls_back_to_the_ranges_own_label_without_a_server_day() {
    // A server too old to send `as_of_day` leaves nothing to date the window
    // against — better the range's plain name than a date the client invented.
    assert_eq!(
        window_label(StatsRange::Month, ""),
        StatsRange::Month.label()
    );
    assert_eq!(
        window_label(StatsRange::Week, "not-a-day"),
        StatsRange::Week.label()
    );
}

#[test]
fn freshness_note_text_states_the_real_ttl_in_seconds() {
    assert_eq!(
        freshness_note_text(),
        format!("Stats are accurate to the last ~{STATS_TTL_SECS} seconds.")
    );
}
