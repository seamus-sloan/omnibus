//! Tests for the small display formatters in [`super`].

use super::*;

fn file(format: &str, ordinal: i64, label: Option<&str>) -> BookFileInfo {
    BookFileInfo {
        id: 1,
        format: format.to_string(),
        filename: "f".into(),
        ordinal,
        label: label.map(str::to_string),
        size_bytes: 0,
        path: None,
        etag: None,
        duration_seconds: None,
    }
}

#[test]
fn file_size_scales_the_unit_to_the_byte_count() {
    assert_eq!(file_size(512), Some("512 B".into()));
    assert_eq!(file_size(3_100), Some("3.1 KB".into()));
    assert_eq!(file_size(3_100_000), Some("3.1 MB".into()));
    assert_eq!(file_size(2_500_000_000), Some("2.5 GB".into()));
}

#[test]
fn file_size_is_absent_for_an_unstated_size() {
    assert_eq!(file_size(0), None);
    assert_eq!(file_size(-1), None);
}

#[test]
fn file_label_prefers_the_stored_label_over_the_ordinal() {
    assert_eq!(
        file_label(&file("epub", 0, Some("10th anniversary"))),
        "EPUB · 10th anniversary"
    );
}

#[test]
fn file_label_falls_back_to_a_one_based_part_number() {
    assert_eq!(file_label(&file("mp3", 1, None)), "MP3 · Part 2");
    assert_eq!(file_label(&file("mp3", 1, Some("  "))), "MP3 · Part 2");
}

#[test]
fn plural_matches_count() {
    assert_eq!(plural(0), "s");
    assert_eq!(plural(1), "");
    assert_eq!(plural(2), "s");
}

#[test]
fn plural_noun_is_singular_only_at_exactly_one() {
    assert_eq!(plural_noun(0, "day"), "days");
    assert_eq!(plural_noun(1, "day"), "day");
    assert_eq!(plural_noun(2, "day"), "days");
}

#[test]
fn count_label_pairs_the_count_with_a_matching_noun() {
    assert_eq!(count_label(1, "session"), "1 session");
    assert_eq!(count_label(4, "session"), "4 sessions");
    assert_eq!(count_label(0, "session"), "0 sessions");
}

#[test]
fn format_date_short_renders_a_full_iso_timestamp_with_offset() {
    assert_eq!(
        format_date_short("2016-05-02T21:00:00+00:00"),
        "May 2nd, 2016"
    );
}

#[test]
fn format_date_short_renders_a_full_iso_timestamp_with_z_suffix() {
    assert_eq!(format_date_short("2026-07-31T00:01:35Z"), "Jul 31st, 2026");
}

#[test]
fn format_date_short_renders_the_sqlite_datetime_shape_added_at_uses() {
    assert_eq!(format_date_short("2024-01-02 03:04:05"), "Jan 2nd, 2024");
}

#[test]
fn format_date_short_renders_a_bare_calendar_date() {
    assert_eq!(format_date_short("1843-10-01"), "Oct 1st, 1843");
}

#[test]
fn format_date_short_falls_back_to_month_and_year_with_no_day() {
    assert_eq!(format_date_short("2016-05"), "May 2016");
}

#[test]
fn format_date_short_falls_back_to_the_bare_year_with_no_month() {
    assert_eq!(format_date_short("2016"), "2016");
}

#[test]
fn format_date_short_narrows_to_month_and_year_when_the_day_is_out_of_range() {
    assert_eq!(format_date_short("2016-05-00"), "May 2016");
    assert_eq!(format_date_short("2016-05-32"), "May 2016");
}

#[test]
fn format_date_short_renders_an_em_dash_for_an_empty_string() {
    assert_eq!(format_date_short(""), "\u{2014}");
}

#[test]
fn format_date_short_renders_an_em_dash_for_unparsable_text() {
    assert_eq!(format_date_short("circa 1850"), "\u{2014}");
}

#[test]
fn format_date_short_renders_an_em_dash_for_the_calibre_undefined_date_sentinel() {
    assert_eq!(format_date_short("0101-01-01T00:00:00+00:00"), "\u{2014}");
}

#[test]
fn format_date_short_renders_an_em_dash_for_year_at_or_below_the_sentinel() {
    assert_eq!(format_date_short("0001-01-01"), "\u{2014}");
    assert_eq!(format_date_short("0101-06-15"), "\u{2014}");
}

#[test]
fn format_date_short_renders_a_real_year_just_above_the_sentinel() {
    assert_eq!(format_date_short("0102-01-01"), "Jan 1st, 102");
}

#[test]
fn format_date_short_ordinalizes_every_day_of_the_month() {
    // The 11th/12th/13th are the cases a naive last-digit rule renders as
    // "11st"/"12nd"/"13rd".
    assert_eq!(format_date_short("2026-08-01"), "Aug 1st, 2026");
    assert_eq!(format_date_short("2026-08-02"), "Aug 2nd, 2026");
    assert_eq!(format_date_short("2026-08-03"), "Aug 3rd, 2026");
    assert_eq!(format_date_short("2026-08-04"), "Aug 4th, 2026");
    assert_eq!(format_date_short("2026-08-11"), "Aug 11th, 2026");
    assert_eq!(format_date_short("2026-08-12"), "Aug 12th, 2026");
    assert_eq!(format_date_short("2026-08-13"), "Aug 13th, 2026");
    assert_eq!(format_date_short("2026-08-21"), "Aug 21st, 2026");
    assert_eq!(format_date_short("2026-08-22"), "Aug 22nd, 2026");
    assert_eq!(format_date_short("2026-08-23"), "Aug 23rd, 2026");
    assert_eq!(format_date_short("2026-08-31"), "Aug 31st, 2026");
}

#[test]
fn format_year_renders_the_bare_year_of_a_full_timestamp() {
    assert_eq!(
        format_year("2015-01-01T05:00:00+00:00"),
        Some("2015".to_string())
    );
    assert_eq!(format_year("2016-05"), Some("2016".to_string()));
    assert_eq!(format_year("2016"), Some("2016".to_string()));
}

#[test]
fn format_year_treats_the_calibre_undefined_date_sentinel_as_absent() {
    // The hero kicker's reported "· 0101": a raw `published.get(0..4)`
    // rendered the placeholder year as if it were a real one.
    assert_eq!(format_year("0101-01-01T00:00:00+00:00"), None);
    assert_eq!(format_year("0001-01-01"), None);
}

#[test]
fn format_year_is_absent_for_empty_and_unparsable_text() {
    assert_eq!(format_year(""), None);
    assert_eq!(format_year("circa 1850"), None);
}

#[test]
fn format_year_agrees_with_format_date_short_on_whether_a_date_exists() {
    // AC3 of #2244: the kicker and the table cell must never disagree about
    // the same book's date.
    for raw in [
        "2015-01-01T05:00:00+00:00",
        "0101-01-01T00:00:00+00:00",
        "circa 1850",
        "",
        "2016",
    ] {
        assert_eq!(
            format_year(raw).is_some(),
            format_date_short(raw) != "\u{2014}",
            "disagreed on {raw:?}"
        );
    }
}

#[test]
fn format_date_month_year_renders_month_and_year() {
    assert_eq!(
        format_date_month_year("2016-05-02T21:00:00+00:00"),
        "May 2016"
    );
}

#[test]
fn format_date_month_year_falls_back_to_the_bare_year_with_no_month() {
    assert_eq!(format_date_month_year("2016"), "2016");
}

#[test]
fn format_date_month_year_renders_an_em_dash_for_the_sentinel() {
    assert_eq!(
        format_date_month_year("0101-01-01T00:00:00+00:00"),
        "\u{2014}"
    );
}

#[test]
fn format_date_month_year_renders_an_em_dash_for_an_empty_string() {
    assert_eq!(format_date_month_year(""), "\u{2014}");
}

#[test]
fn format_date_month_year_opt_is_some_for_a_real_date() {
    assert_eq!(
        format_date_month_year_opt("2016-05-02"),
        Some("May 2016".to_string())
    );
    assert_eq!(format_date_month_year_opt("2016"), Some("2016".to_string()));
}

#[test]
fn format_date_month_year_opt_is_none_for_absent_and_sentinel_dates() {
    // The series card drops the whole slot on `None`, so a sentinel and a
    // truly-absent date must both answer `None` — otherwise one renders `· —`
    // and the other nothing (#2294, #2360).
    assert_eq!(format_date_month_year_opt(""), None);
    assert_eq!(format_date_month_year_opt("circa 1850"), None);
    assert_eq!(
        format_date_month_year_opt("0101-01-01T00:00:00+00:00"),
        None
    );
}

// Regression for #2504: `Science Fiction & Fantasy` used to become four
// facets — `tag:Science tag:Fiction tag:& tag:Fantasy` — AND-ed, which is a
// different question from the one the reader clicked.
#[test]
fn facet_query_quotes_a_multi_word_name_into_one_facet() {
    assert_eq!(facet_query("tag", "Dark academia"), "tag:\"Dark academia\"");
    assert_eq!(
        facet_query("genre", "Science Fiction & Fantasy"),
        "genre:\"Science Fiction & Fantasy\""
    );
}

#[test]
fn facet_query_passes_a_single_word_through_unquoted() {
    // Nothing to protect, and the shorter URL is the one a reader may edit.
    assert_eq!(facet_query("genre", "Horror"), "genre:Horror");
}

#[test]
fn facet_query_escapes_an_embedded_quote_rather_than_ending_the_value() {
    assert_eq!(
        facet_query("tag", "the \"good\" parts"),
        "tag:\"the \"\"good\"\" parts\""
    );
}

#[test]
fn facet_query_collapses_surrounding_whitespace_and_declines_an_empty_name() {
    assert_eq!(facet_query("tag", "  Horror  "), "tag:Horror");
    assert_eq!(facet_query("tag", "   "), "");
    assert_eq!(facet_query("tag", ""), "");
}

// --- the inverse, for the results heading -------------------------------

#[test]
fn single_facet_value_reads_back_what_facet_query_wrote() {
    for (prefix, name) in [
        ("tag", "Science Fiction & Fantasy"),
        ("genre", "Horror"),
        ("author", "Ursula K. Le Guin"),
        ("series", "The Broken Earth"),
        ("tag", "the \"good\" parts"),
    ] {
        assert_eq!(
            single_facet_value(&facet_query(prefix, name)).as_deref(),
            Some(name),
            "{prefix}:{name}"
        );
    }
}

#[test]
fn single_facet_value_declines_anything_that_is_not_one_lone_facet() {
    // Free text is the reader's own words — echo them, don't rewrite them.
    assert_eq!(single_facet_value("science fiction"), None);
    // Two facets, or a facet plus text, are not "the name they clicked".
    assert_eq!(single_facet_value("tag:Horror genre:Fiction"), None);
    assert_eq!(single_facet_value("tag:Horror ghosts"), None);
    assert_eq!(single_facet_value("tag:\"Dark academia\" more"), None);
    // An unknown prefix is not a facet at all.
    assert_eq!(single_facet_value("http://example.com"), None);
    // And an empty value names nothing.
    assert_eq!(single_facet_value("tag:"), None);
    assert_eq!(single_facet_value("tag:\"\""), None);
}

// --- instants vs calendar dates (#2464) ---------------------------------

#[test]
fn format_instant_short_opt_matches_the_plain_render_at_utc() {
    // Offset 0 is the SSR / pre-hydration path (rule 07), so the two must
    // agree glyph for glyph or hydration would swap the row.
    let raw = "2026-09-08T03:36:00Z";
    assert_eq!(
        format_instant_short_opt(raw, 0).as_deref(),
        format_date_short_opt(raw).as_deref()
    );
}

// Regression for #2464: "Added Sep 8th" on a page whose every other stamp
// read Sep 7, because 03:36 UTC is the previous evening in Detroit.
#[test]
fn format_instant_short_opt_dates_an_instant_on_the_viewers_day() {
    assert_eq!(
        format_instant_short_opt("2026-09-08T03:36:00Z", -4 * 3600).as_deref(),
        Some("Sep 7th, 2026")
    );
    // And forward across midnight the other way.
    assert_eq!(
        format_instant_short_opt("2026-09-07T22:10:00Z", 4 * 3600).as_deref(),
        Some("Sep 8th, 2026")
    );
}

#[test]
fn format_instant_short_opt_reads_the_sqlite_datetime_shape() {
    assert_eq!(
        format_instant_short_opt("2024-01-02 03:04:05", -5 * 3600).as_deref(),
        Some("Jan 1st, 2024")
    );
}

#[test]
fn format_instant_short_opt_leaves_a_bare_calendar_date_unshifted() {
    // A publication date carries no time of day and belongs to no zone;
    // shifting it would move a book's publication a day for half the world.
    for offset in [-12 * 3600, 0, 12 * 3600] {
        assert_eq!(
            format_instant_short_opt("2016-05-02", offset).as_deref(),
            Some("May 2nd, 2016"),
            "offset {offset}"
        );
    }
}

#[test]
fn format_instant_short_opt_keeps_the_sentinel_and_unparseable_gates() {
    // Calibre's UNDEFINED_DATE, and a string that names no year at all.
    assert_eq!(
        format_instant_short_opt("0101-01-01T00:00:00+00:00", 0),
        None
    );
    assert_eq!(format_instant_short_opt("", 0), None);
    assert_eq!(format_instant_short_opt("not a date", 0), None);
}

#[test]
fn instant_secs_resolves_a_timestamp_and_declines_a_bare_date() {
    assert_eq!(instant_secs("1970-01-01T00:00:00Z"), Some(0));
    assert_eq!(instant_secs("2023-11-14T22:13:20Z"), Some(1_700_000_000));
    // No time of day: there is no instant here to resolve.
    assert_eq!(instant_secs("2023-11-14"), None);
}
