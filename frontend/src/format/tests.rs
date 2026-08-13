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
fn format_date_short_renders_a_full_iso_timestamp_with_offset() {
    assert_eq!(
        format_date_short("2016-05-02T21:00:00+00:00"),
        "May 2, 2016"
    );
}

#[test]
fn format_date_short_renders_a_full_iso_timestamp_with_z_suffix() {
    assert_eq!(format_date_short("2026-07-31T00:01:35Z"), "July 31, 2026");
}

#[test]
fn format_date_short_renders_the_sqlite_datetime_shape_added_at_uses() {
    assert_eq!(format_date_short("2024-01-02 03:04:05"), "January 2, 2024");
}

#[test]
fn format_date_short_renders_a_bare_calendar_date() {
    assert_eq!(format_date_short("1843-10-01"), "October 1, 1843");
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
    assert_eq!(format_date_short("0102-01-01"), "January 1, 102");
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
