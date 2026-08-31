use super::*;
use omnibus_shared::PagesReadDetail;

fn summary(range: StatsRange) -> StatsSummary {
    StatsSummary {
        range,
        ..StatsSummary::default()
    }
}

#[test]
fn duration_value_scales_minutes_decimal_hours_and_whole_hours() {
    assert_eq!(duration_value(0), ("0".into(), "min"));
    assert_eq!(duration_value(42 * 60), ("42".into(), "min"));
    assert_eq!(duration_value(3 * 3600 + 1800), ("3.5".into(), "hours"));
    assert_eq!(duration_value(142 * 3600 + 120), ("142".into(), "hours"));
}

#[test]
fn avg_stars_value_rounds_half_up_to_one_decimal_or_em_dash() {
    // 4.25 rounds half away from zero → 4.3 (not {:.1}'s half-to-even 4.2).
    assert_eq!(avg_stars_value(Some(4.25)), "4.3");
    assert_eq!(avg_stars_value(Some(4.24)), "4.2");
    assert_eq!(avg_stars_value(Some(5.0)), "5.0");
    assert_eq!(avg_stars_value(None), "\u{2014}");
}

#[test]
fn pages_value_groups_thousands_or_shows_em_dash() {
    assert_eq!(pages_value(Some(0), false), "0");
    assert_eq!(pages_value(Some(214), false), "214");
    assert_eq!(pages_value(Some(9214), false), "9,214");
    assert_eq!(pages_value(Some(1_234_567), false), "1,234,567");
    assert_eq!(pages_value(None, false), "\u{2014}");
}

#[test]
fn pages_value_reads_an_audio_only_window_as_zero_not_as_unknown() {
    // Listening turns no pages, which is a fact the tile can state; the
    // em-dash would claim the server doesn't know what happened.
    assert_eq!(pages_value(None, true), "0");
    // A measured total always wins over the empty-state branch.
    assert_eq!(pages_value(Some(31), true), "31");
}

#[test]
fn fill_pct_reaches_the_baseline_and_pegs_once_past_it() {
    assert_eq!(fill_pct(0.0, 100.0), 0);
    assert_eq!(fill_pct(50.0, 100.0), 50);
    assert_eq!(fill_pct(100.0, 100.0), 100);
    // Past the baseline the bar is full, not overflowing.
    assert_eq!(fill_pct(400.0, 100.0), 100);
    // Nothing to measure against: a figure fills, an absence stays empty.
    assert_eq!(fill_pct(9.0, 0.0), 100);
    assert_eq!(fill_pct(0.0, 0.0), 0);
}

#[test]
fn count_comparison_states_whole_units_and_names_a_flat_window() {
    assert_eq!(count_comparison(6, 4).label, "+2");
    assert_eq!(count_comparison(3, 4).label, "\u{2212}1");
    assert_eq!(count_comparison(4, 4).label, "flat");
    assert_eq!(count_comparison(6, 4).css_class, "up");
    assert_eq!(count_comparison(3, 4).css_class, "down");
    assert_eq!(count_comparison(4, 4).css_class, "flat");
}

#[test]
fn percent_comparison_scales_the_change_and_calls_an_empty_baseline_new() {
    assert_eq!(percent_comparison(118.0, 100.0).label, "+18%");
    assert_eq!(percent_comparison(94.0, 100.0).label, "\u{2212}6%");
    // Under half a percent is not a change worth reporting.
    assert_eq!(percent_comparison(100.4, 100.0).label, "flat");
    assert_eq!(percent_comparison(31.0, 0.0).label, "new");
    // Nothing either side is flat, not new.
    assert_eq!(percent_comparison(0.0, 0.0).label, "flat");
}

#[test]
fn stars_comparison_draws_against_the_five_star_ceiling_not_the_last_window() {
    // A window that rated exactly as well as the last still shows how good
    // 4.0 is; a ratio between the two means would peg the bar at full.
    let same = stars_comparison(Some(4.0), Some(4.0)).expect("a rated window");
    assert_eq!(same.label, "flat");
    assert_eq!(same.fill_pct, 80);

    let better = stars_comparison(Some(4.3), Some(4.1)).expect("a rated window");
    assert_eq!(better.label, "+0.2");
    assert_eq!(better.css_class, "up");
    let worse = stars_comparison(Some(3.9), Some(4.1)).expect("a rated window");
    assert_eq!(worse.label, "\u{2212}0.2");
    assert_eq!(worse.css_class, "down");
}

#[test]
fn stars_comparison_is_absent_without_a_mean_to_report() {
    // A mean over nothing is not zero — the tile shows an em-dash and no
    // comparison at all rather than claiming a rating fell to nothing.
    assert!(stars_comparison(None, Some(4.0)).is_none());
    // A first rated window has a figure but no baseline.
    assert_eq!(
        stars_comparison(Some(4.0), None).map(|c| c.label),
        Some("new".to_string())
    );
}

#[test]
fn build_tiles_drops_every_comparison_on_lifetime() {
    // `PeriodComparison` is `Default` for Lifetime, so a delta drawn against
    // it would report a reader's whole history as brand new.
    let mut all_time = summary(StatsRange::AllTime);
    all_time.books_finished = 264;
    all_time.avg_stars = Some(4.1);
    all_time.listening_seconds = 3_600;
    all_time.pages_read = Some(92_410);

    let tiles = build_tiles(&all_time);
    assert_eq!(tiles.len(), 4);
    assert!(tiles.iter().all(|t| t.comparison.is_none()));
    // The figures themselves still render.
    assert_eq!(tiles[0].value, "264");
    assert_eq!(tiles[1].value, "92,410");
}

#[test]
fn build_tiles_compares_every_metric_on_a_bounded_window() {
    let mut month = summary(StatsRange::Month);
    month.books_finished = 4;
    month.previous.books_finished = 2;
    month.pages_read = Some(1_284);
    month.previous.pages_read = 1_088;
    month.listening_seconds = 41_040;
    month.previous.listening_seconds = 43_660;
    month.avg_stars = Some(4.3);
    month.previous.avg_stars = Some(4.3);
    month.pages_detail = PagesReadDetail {
        measured_books: 5,
        ..PagesReadDetail::default()
    };

    let tiles = build_tiles(&month);
    let label = |i: usize| {
        tiles[i]
            .comparison
            .as_ref()
            .map(|c| c.label.clone())
            .unwrap_or_default()
    };
    assert_eq!(label(0), "+2");
    assert_eq!(label(1), "+18%");
    assert_eq!(label(2), "\u{2212}6%");
    assert_eq!(label(3), "flat");
}
