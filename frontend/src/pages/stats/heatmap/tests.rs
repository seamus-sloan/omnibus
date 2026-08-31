use super::*;

fn day(n: i64) -> i64 {
    day_number(&format!("2026-07-{n:02}")).unwrap_or(0)
}

#[test]
fn day_number_round_trips_a_civil_date_and_rejects_a_malformed_one() {
    let n = day_number("2026-07-12").expect("a real day");
    assert_eq!(civil_from_days(n), (2026, 7, 12));
    assert_eq!(day_number("2026-13-01"), None);
    assert_eq!(day_number("2026-07-32"), None);
    assert_eq!(day_number("not-a-day"), None);
    assert_eq!(day_number(""), None);
}

#[test]
fn streak_span_measures_back_from_the_last_active_day_not_from_today() {
    // A run that ended yesterday is still live — today isn't over. Measuring
    // back from the anchor would outline one cell too many and leave the grid
    // disagreeing with the figure reporting the run.
    let (from, to) = streak_span(day(12), &[day(9), day(10), day(11)], 3).expect("a live run");
    assert_eq!(from, day(9));
    assert_eq!(to, day(11));
}

#[test]
fn streak_span_is_none_without_a_live_run_or_any_activity() {
    assert_eq!(streak_span(day(12), &[day(12)], 0), None);
    assert_eq!(streak_span(day(12), &[], 4), None);
    // Activity that is all in the future of the anchor can't end a run.
    assert_eq!(streak_span(day(12), &[day(20)], 3), None);
}

#[test]
fn coverage_measures_against_the_days_elapsed_not_the_whole_grid() {
    // A year is not over. Dividing by 364 would report every reader's
    // December as a failure to have read the days that haven't happened.
    let start = day(1);
    let anchor = day(10);
    let by_day: HashMap<i64, i64> = (1..=5).map(|n| (day(n), 600)).collect();
    assert_eq!(coverage(&by_day, start, anchor), (5, 50));

    // Nothing recorded is zero of both, never a divide by zero.
    assert_eq!(coverage(&HashMap::new(), start, anchor), (0, 0));
}

#[test]
fn coverage_ignores_days_outside_the_drawn_window() {
    let by_day: HashMap<i64, i64> = [(day(1) - 40, 600), (day(3), 600)].into_iter().collect();
    assert_eq!(coverage(&by_day, day(1), day(4)), (1, 25));
}

#[test]
fn intensity_reserves_zero_for_a_quiet_day_and_caps_at_four() {
    assert_eq!(intensity(0, 100), 0);
    assert_eq!(intensity(50, 0), 0);
    assert_eq!(intensity(1, 100), 1);
    assert_eq!(intensity(100, 100), 4);
    assert_eq!(intensity(400, 100), 4);
}

#[test]
fn build_cells_outlines_only_the_live_run_and_only_where_something_happened() {
    let anchor = day(12);
    let by_day: HashMap<i64, i64> = [
        (day(10), 600),
        (day(11), 600),
        (day(12), 600),
        (day(3), 600),
    ]
    .into_iter()
    .collect();
    let streak = streak_span(anchor, &[day(10), day(11), day(12)], 3);
    let cells = build_cells(anchor, &by_day, 600, streak);

    let outlined: Vec<&str> = cells
        .iter()
        .filter(|c| c.in_streak)
        .map(|c| c.day.as_str())
        .collect();
    assert_eq!(outlined, ["2026-07-10", "2026-07-11", "2026-07-12"]);
    // The earlier active day is drawn, but not as part of the run.
    let earlier = cells.iter().find(|c| c.day == "2026-07-03").expect("drawn");
    assert!(earlier.secs > 0 && !earlier.in_streak);
}

#[test]
fn build_cells_marks_the_days_after_the_anchor_as_spacers() {
    let anchor = day(8); // a Wednesday, so the final week runs past it
    let cells = build_cells(anchor, &HashMap::new(), 0, None);
    assert_eq!(cells.len() as i64, WEEKS * 7);
    assert!(cells.iter().any(|c| c.future));
    assert!(cells.iter().filter(|c| c.future).all(|c| c.secs == 0));
}

#[test]
fn format_active_time_drops_an_empty_component() {
    assert_eq!(format_active_time(42 * 60), "42 m");
    assert_eq!(format_active_time(3 * 3600), "3 h");
    assert_eq!(format_active_time(3 * 3600 + 20 * 60), "3 h 20 m");
}

#[test]
fn trailing_month_labels_end_at_the_anchors_own_month() {
    let labels = trailing_month_labels(day_number("2026-08-14").expect("a real day"));
    assert_eq!(labels.len(), 12);
    assert_eq!(labels.first().copied(), Some("Sep"));
    assert_eq!(labels.last().copied(), Some("Aug"));
}

/// The record stands alone in this card's header: the run the reader is *on*
/// leads the hero, and a second copy two bands apart is where two surfaces
/// start to disagree about the same figure.
#[cfg(feature = "server")]
#[test]
fn heatmap_card_reports_the_record_and_leaves_the_live_run_to_the_hero() {
    let summary = StatsSummary {
        as_of_day: "2026-07-12".to_string(),
        current_streak_days: 3,
        longest_streak_days: 9,
        ..Default::default()
    };
    let html = crate::test_support::render(rsx! { HeatmapCard { summary } });

    assert!(html.contains("stats-longest-streak"), "{html}");
    assert!(html.contains("best run"), "{html}");
    assert!(!html.contains("stats-current-streak"), "{html}");
}

/// Issue #2250: the record's *unit* pluralizes on its own figure — "1 day",
/// "9 days" — while the label stays "best run", because the figure is how long
/// the run was and not how many runs there were.
#[cfg(feature = "server")]
#[test]
fn heatmap_card_pluralizes_the_records_unit_but_never_its_label() {
    let one = crate::test_support::render(rsx! {
        HeatmapCard { summary: StatsSummary {
            as_of_day: "2026-07-12".to_string(),
            longest_streak_days: 1,
            ..Default::default()
        } }
    });
    assert!(one.contains(" day<"), "{one}");
    assert!(one.contains(">best run<"), "{one}");

    let many = crate::test_support::render(rsx! {
        HeatmapCard { summary: StatsSummary {
            as_of_day: "2026-07-12".to_string(),
            longest_streak_days: 9,
            ..Default::default()
        } }
    });
    assert!(many.contains(" days<"), "{many}");
    assert!(many.contains(">best run<"), "{many}");
}

/// A summary with no `as_of_day` and no activity has no anchor to draw
/// against — the card holds its space rather than rendering a grid of days it
/// would have to invent.
#[cfg(feature = "server")]
#[test]
fn heatmap_card_holds_its_space_when_there_is_no_day_to_anchor_on() {
    let html = crate::test_support::render(rsx! {
        HeatmapCard { summary: StatsSummary::default() }
    });
    assert!(html.contains("st-card-placeholder"), "{html}");
    assert!(!html.contains("stats-heatmap"), "{html}");
}
