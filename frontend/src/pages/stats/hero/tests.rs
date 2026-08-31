use super::*;
use omnibus_shared::DayActivity;

fn summary(as_of: &str, streak: i64, days: &[(&str, i64)]) -> StatsSummary {
    StatsSummary {
        as_of_day: as_of.to_string(),
        current_streak_days: streak,
        heatmap: days
            .iter()
            .map(|(day, secs)| DayActivity {
                day: (*day).to_string(),
                seconds: *secs,
            })
            .collect(),
        ..StatsSummary::default()
    }
}

#[test]
fn build_spark_draws_one_slot_per_day_and_scales_to_its_own_busiest() {
    let s = summary(
        "2026-08-12",
        2,
        &[
            ("2026-08-12", 1_800),
            ("2026-08-11", 3_600),
            ("2026-08-01", 900),
        ],
    );
    let anchor = day_number("2026-08-12").expect("a real day");
    let bars = build_spark(&s, anchor);

    assert_eq!(bars.len() as i64, SPARK_DAYS);
    let last = bars.last().expect("a bar per day");
    assert_eq!(last.day, "2026-08-12");
    assert_eq!(last.height_pct, 50, "half the busiest day drawn");
    // The quiet days keep their slots — the gaps are what make a run visible.
    assert!(bars.iter().filter(|b| !b.active).count() > 30);
}

#[test]
fn build_spark_marks_only_the_live_run_and_only_where_something_happened() {
    let s = summary(
        "2026-08-12",
        2,
        &[
            ("2026-08-12", 600),
            ("2026-08-11", 600),
            ("2026-08-05", 600),
        ],
    );
    let anchor = day_number("2026-08-12").expect("a real day");
    let bars = build_spark(&s, anchor);

    let lit: Vec<&str> = bars
        .iter()
        .filter(|b| b.in_streak)
        .map(|b| b.day.as_str())
        .collect();
    assert_eq!(lit, ["2026-08-11", "2026-08-12"]);
    // The earlier active day is drawn, but not as part of the run.
    let earlier = bars.iter().find(|b| b.day == "2026-08-05").expect("drawn");
    assert!(earlier.active && !earlier.in_streak);
}

#[test]
fn build_spark_stays_flat_when_nothing_was_recorded() {
    let s = summary("2026-08-12", 0, &[]);
    let anchor = day_number("2026-08-12").expect("a real day");
    let bars = build_spark(&s, anchor);
    assert!(bars.iter().all(|b| b.height_pct == 0 && !b.active));
}

#[test]
fn streak_line_compares_the_run_against_the_record_and_says_nothing_without_one() {
    assert_eq!(
        streak_line(3, 61).as_deref(),
        Some("Your longest ever is 61 days.")
    );
    assert_eq!(
        streak_line(0, 1).as_deref(),
        Some("Your longest ever is 1 day.")
    );
    assert_eq!(
        streak_line(61, 61).as_deref(),
        Some("That is the longest run you have recorded.")
    );
    // Nothing recorded yet: the sentence would be furniture.
    assert_eq!(streak_line(0, 0), None);
}
