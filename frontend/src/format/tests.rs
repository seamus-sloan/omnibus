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

#[test]
fn facet_query_scopes_every_word_of_a_multi_word_name() {
    // The bug this replaced: `format!("tag:{name}")` scoped only "Dark" and
    // let "academia" fall through to `build_fts_match`'s free-text arm, so a
    // chip click matched books merely titled something with "academia".
    assert_eq!(facet_query("tag", "Dark academia"), "tag:Dark tag:academia");
    assert_eq!(
        facet_query("genre", "Hard Science Fiction"),
        "genre:Hard genre:Science genre:Fiction"
    );
}

#[test]
fn facet_query_passes_a_single_word_through_unchanged() {
    assert_eq!(facet_query("genre", "Horror"), "genre:Horror");
}

#[test]
fn facet_query_collapses_surrounding_and_repeated_whitespace() {
    // `split_whitespace` drops empties, so no `tag:` token is ever emitted
    // bare — `build_fts_match` would silently discard one.
    assert_eq!(
        facet_query("tag", "  Dark   academia  "),
        "tag:Dark tag:academia"
    );
    assert_eq!(facet_query("tag", "   "), "");
}
