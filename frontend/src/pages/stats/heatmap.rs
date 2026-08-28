//! Reading-days heatmap card: a pure-CSS GitHub-style trailing-year calendar
//! grid (in the all-time section, never re-queried by the switcher) with the
//! longest-streak figure in its header. Day math runs on epoch-day numbers via
//! the civil-date algorithms below — no date crate — over the DTO's UTC
//! `YYYY-MM-DD` strings.

use std::collections::HashMap;

use dioxus::prelude::*;
use omnibus_shared::StatsSummary;

/// Weeks of trailing history the grid renders.
const WEEKS: i64 = 52;

/// Days since 1970-01-01 for a civil date (Howard Hinnant's `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)` for an epoch day.
fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = (mp + 2) % 12 + 1;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Epoch-day number for a UTC `YYYY-MM-DD` string, `None` when malformed.
fn day_number(day: &str) -> Option<i64> {
    let mut parts = day.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// `YYYY-MM-DD` for an epoch-day number.
fn day_string(n: i64) -> String {
    let (y, m, d) = civil_from_days(n);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Intensity bucket 0..=4 — zero maps to 0, anything active lands 1..=4
/// relative to the window's busiest day.
fn intensity(secs: i64, max: i64) -> u8 {
    if secs <= 0 || max <= 0 {
        return 0;
    }
    u8::try_from((1 + (secs.saturating_mul(4).saturating_sub(1)) / max).min(4)).unwrap_or(4)
}

/// Human duration for cell tooltips: "42 m", "3 h", "3 h 20 m".
fn format_active_time(secs: i64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// Three-letter month name for a 1-based month number.
fn month_abbr(m: i64) -> &'static str {
    match m {
        1 => "Jan",
        2 => "Feb",
        3 => "Mar",
        4 => "Apr",
        5 => "May",
        6 => "Jun",
        7 => "Jul",
        8 => "Aug",
        9 => "Sep",
        10 => "Oct",
        11 => "Nov",
        _ => "Dec",
    }
}

/// The trailing 12 month labels ending at (and including) `anchor`'s month.
fn trailing_month_labels(anchor: i64) -> Vec<&'static str> {
    let (y, m, _) = civil_from_days(anchor);
    (0..12)
        .rev()
        .map(|back| {
            let idx = (y * 12 + (m - 1)) - back;
            month_abbr(idx.rem_euclid(12) + 1)
        })
        .collect()
}

/// One rendered cell: its day, seconds, and intensity bucket.
struct HeatCell {
    day: String,
    secs: i64,
    level: u8,
    /// Days after the anchor in the final week render as invisible spacers.
    future: bool,
}

/// Epoch day of the grid's first cell: the Monday of `anchor`'s week, 51 weeks
/// back. Unix day 0 is a Thursday, so Monday-aligning subtracts `(n + 3) % 7`
/// — the same convention as `db::stats`' busiest-week bucketing.
///
/// Shared with the caller's `by_day` filter so the intensity scale is
/// normalized over exactly the days the grid draws. A fixed `anchor - 363`
/// kept up to six days falling before the first Monday, and `max` comes from
/// those values — so one heavy day just off the left edge flattened every
/// visible cell against a peak the reader could not see.
fn grid_start(anchor: i64) -> i64 {
    let monday = anchor - (anchor + 3).rem_euclid(7);
    monday - (WEEKS - 1) * 7
}

/// Build the cell list: 52 week columns of 7 rows (Monday-first), ending in
/// the week containing `anchor`.
fn build_cells(anchor: i64, by_day: &HashMap<i64, i64>, max: i64) -> Vec<HeatCell> {
    let start = grid_start(anchor);
    (start..start + WEEKS * 7)
        .map(|n| {
            let secs = by_day.get(&n).copied().unwrap_or(0);
            HeatCell {
                day: day_string(n),
                secs,
                level: intensity(secs, max),
                future: n > anchor,
            }
        })
        .collect()
}

/// The all-time heatmap card: header with the longest-streak figure, the
/// calendar grid, month labels, and the less/more legend.
#[component]
pub(super) fn HeatmapCard(summary: StatsSummary) -> Element {
    // Anchor to the server's clock; fall back to the newest active day so a
    // missing stamp still renders something sensible.
    let anchor = day_number(&summary.as_of_day).or_else(|| {
        summary
            .heatmap
            .iter()
            .filter_map(|d| day_number(&d.day))
            .max()
    });
    let Some(anchor) = anchor else {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    };

    let window_start = grid_start(anchor);
    let by_day: HashMap<i64, i64> = summary
        .heatmap
        .iter()
        .filter_map(|d| day_number(&d.day).map(|n| (n, d.seconds)))
        .filter(|(n, _)| *n >= window_start)
        .collect();
    let max = by_day.values().copied().max().unwrap_or(0);
    let cells = build_cells(anchor, &by_day, max);
    let months = trailing_month_labels(anchor);
    let streak = summary.longest_streak_days;

    rsx! {
        div { class: "card st-heatmap-card", "data-testid": "stats-heatmap",
            div { class: "st-heatmap-head",
                div {
                    div { class: "label", "Reading days \u{00B7} trailing year" }
                    div { class: "st-heatmap-sub", "Longest run of consecutive days you read." }
                }
                div { class: "st-streak",
                    div { class: "st-streak-value",
                        "{streak} "
                        span { class: "st-streak-unit", "days" }
                    }
                    div { class: "label", "Longest streak" }
                }
            }
            // Block-level scroll wrapper: iOS WebKit clips this far more reliably than a grid-as-scroller (#1076).
            div { class: "st-heatmap-scroll",
                div { class: "st-heatmap", role: "img", aria_label: "Daily reading activity, trailing year",
                    for cell in cells {
                        div {
                            key: "{cell.day}",
                            class: if cell.future { "st-hm-cell st-hm-future" } else { "st-hm-cell st-hm-{cell.level}" },
                            title: if !cell.future && cell.secs > 0 { "{format_active_time(cell.secs)} on {cell.day}" } else { "{cell.day}" },
                        }
                    }
                }
            }
            div { class: "st-hm-months", aria_hidden: "true",
                for m in months {
                    span { key: "{m}", {m} }
                }
            }
            div { class: "st-hm-legend", aria_hidden: "true",
                span { class: "mono", "less" }
                for level in 0..5 {
                    div { key: "{level}", class: "st-hm-cell st-hm-{level}" }
                }
                span { class: "mono", "more" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_number_round_trips_through_day_string() {
        for day in ["1970-01-01", "2026-07-12", "2024-02-29", "1999-12-31"] {
            let n = day_number(day).unwrap();
            assert_eq!(day_string(n), day);
        }
        assert_eq!(day_number("1970-01-01"), Some(0));
        assert_eq!(day_number("not-a-date"), None);
        assert_eq!(day_number("2026-13-01"), None);
    }

    #[test]
    fn intensity_buckets_zero_and_quartiles() {
        assert_eq!(intensity(0, 100), 0);
        assert_eq!(intensity(50, 0), 0);
        assert_eq!(intensity(1, 100), 1);
        assert_eq!(intensity(25, 100), 1);
        assert_eq!(intensity(26, 100), 2);
        assert_eq!(intensity(75, 100), 3);
        assert_eq!(intensity(100, 100), 4);
    }

    #[test]
    fn build_cells_ends_in_the_anchor_week_with_future_spacers() {
        // 2026-07-12 is a Sunday: the final week is fully in the past.
        let anchor = day_number("2026-07-12").unwrap();
        let cells = build_cells(anchor, &HashMap::new(), 0);
        assert_eq!(cells.len(), (WEEKS * 7) as usize);
        assert_eq!(cells.last().unwrap().day, "2026-07-12");
        assert!(cells.iter().all(|c| !c.future));

        // Anchoring midweek (Wednesday) leaves the rest of that week future.
        let wed = day_number("2026-07-08").unwrap();
        let cells = build_cells(wed, &HashMap::new(), 0);
        assert_eq!(cells.iter().filter(|c| c.future).count(), 4);
        assert_eq!(cells.last().unwrap().day, "2026-07-12");
    }

    #[test]
    fn grid_start_is_the_monday_of_the_anchor_week_fifty_one_weeks_back() {
        // 2026-07-06 is a Monday, 2026-07-12 the Sunday that closes its week:
        // both weeks start on the same Monday, so both grids start together.
        let monday = day_number("2026-07-06").unwrap();
        let sunday = day_number("2026-07-12").unwrap();
        assert_eq!(grid_start(monday), grid_start(sunday));
        assert_eq!(day_string(grid_start(sunday)), "2025-07-14");
        // Every rendered cell sits at or after it.
        let cells = build_cells(sunday, &HashMap::new(), 0);
        assert_eq!(cells.first().unwrap().day, day_string(grid_start(sunday)));
    }

    #[test]
    fn trailing_month_labels_end_at_the_anchor_month() {
        let anchor = day_number("2026-07-12").unwrap();
        let months = trailing_month_labels(anchor);
        assert_eq!(months.len(), 12);
        assert_eq!(months[0], "Aug");
        assert_eq!(months[11], "Jul");
    }

    #[test]
    fn format_active_time_covers_minute_hour_and_mixed_spans() {
        assert_eq!(format_active_time(42 * 60), "42 m");
        assert_eq!(format_active_time(3 * 3600), "3 h");
        assert_eq!(format_active_time(3 * 3600 + 20 * 60), "3 h 20 m");
    }
}
