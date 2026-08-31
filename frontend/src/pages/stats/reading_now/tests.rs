use super::*;
use omnibus_shared::{EbookMetadata, ProgressFormat, ProgressRecord};

fn months(pairs: &[(&str, i64)]) -> Vec<MonthCount> {
    pairs
        .iter()
        .map(|(month, books)| MonthCount {
            month: (*month).to_string(),
            books: *books,
        })
        .collect()
}

fn summary(as_of: &str, pairs: &[(&str, i64)]) -> StatsSummary {
    StatsSummary {
        as_of_day: as_of.to_string(),
        books_per_month: months(pairs),
        ..StatsSummary::default()
    }
}

fn point(percent: Option<i64>, chapter: Option<(i64, i64)>) -> ResumePoint {
    ResumePoint {
        record: ProgressRecord {
            book_uuid: "u".to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: percent,
            kobo_location: None,
            book_file_id: None,
            updated_at: 0,
            client_updated_at: 0,
        },
        book: EbookMetadata::default(),
        linked: false,
        cross_format: None,
        total_duration_seconds: None,
        chapter_number: chapter.map(|(n, _)| n),
        chapter_count: chapter.map(|(_, total)| total),
        playback_rate: None,
    }
}

#[test]
fn books_this_year_counts_only_the_current_years_months() {
    // The trailing-12 series always straddles two calendar years; a projection
    // that swept the whole series would count last autumn against this year.
    let series = months(&[
        ("2025-09", 3),
        ("2025-12", 4),
        ("2026-01", 2),
        ("2026-08", 5),
    ]);
    assert_eq!(books_this_year(&series, "2026"), 7);
    assert_eq!(books_this_year(&series, "2025"), 7);
    assert_eq!(books_this_year(&series, "2024"), 0);
    // A server too old to send the day leaves no year to filter on.
    assert_eq!(books_this_year(&series, ""), 0);
}

#[test]
fn year_projection_extrapolates_the_years_pace_to_december() {
    // 2026-07-02 is day 183 of 365 — a shade over half the year — so 15 books
    // projects to about 30.
    let s = summary("2026-07-02", &[("2026-01", 8), ("2026-06", 7)]);
    assert_eq!(year_projection(&s), Some(30));
}

#[test]
fn year_projection_stays_quiet_early_in_the_year_and_with_nothing_finished() {
    // One book by the 5th of January projects seventy-three, which is not a
    // forecast anybody should read.
    let early = summary("2026-01-05", &[("2026-01", 1)]);
    assert_eq!(year_projection(&early), None);
    // Nothing finished has no pace to extend.
    let idle = summary("2026-07-02", &[("2026-01", 0)]);
    assert_eq!(year_projection(&idle), None);
    // And no server day means no year to measure against.
    let undated = summary("", &[("2026-07", 4)]);
    assert_eq!(year_projection(&undated), None);
}

#[test]
fn resume_readout_answers_in_whatever_unit_the_book_can_report() {
    assert_eq!(
        resume_readout(&point(Some(62), None)).as_deref(),
        Some("62%")
    );
    // An audiobook's position is a time offset, not a percent — it answers in
    // chapters instead of inventing one.
    assert_eq!(
        resume_readout(&point(None, Some((3, 12)))).as_deref(),
        Some("Ch 3 of 12")
    );
    // Neither available: silence, not a zero the book never reported.
    assert_eq!(resume_readout(&point(None, None)), None);
}

#[test]
fn resume_percent_clamps_into_the_bar_and_defaults_to_empty() {
    assert_eq!(resume_percent(&point(Some(62), None)), 62);
    assert_eq!(resume_percent(&point(None, Some((3, 12)))), 0);
    assert_eq!(resume_percent(&point(Some(140), None)), 100);
}

#[test]
fn stars_label_fills_to_the_rating_and_em_dashes_an_unrated_book() {
    assert_eq!(
        stars_label(Some(5.0)),
        "\u{2605}\u{2605}\u{2605}\u{2605}\u{2605}"
    );
    assert_eq!(
        stars_label(Some(4.0)),
        "\u{2605}\u{2605}\u{2605}\u{2605}\u{2606}"
    );
    // Five glyphs cannot show a half, so it rounds to the nearer whole star;
    // the drill-in histogram carries the exact distribution.
    assert_eq!(
        stars_label(Some(3.5)),
        "\u{2605}\u{2605}\u{2605}\u{2605}\u{2606}"
    );
    assert_eq!(stars_label(None), "\u{2014}");
}
