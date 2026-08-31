//! "Every day of the last year" — a pure-CSS trailing-year calendar grid, in
//! the standing band so the switcher never re-queries it, with the days-read
//! coverage and the streak record in its header and the live run outlined in
//! the grid. Day math runs on epoch-day numbers via the civil-date algorithms
//! below — no date crate — over the DTO's UTC `YYYY-MM-DD` strings.

use std::collections::HashMap;

use dioxus::prelude::*;
use omnibus_shared::StatsSummary;

use crate::format::plural_noun;

/// Weeks of trailing history the grid renders.
const WEEKS: i64 = 52;

/// Days since 1970-01-01 for a civil date (Howard Hinnant's `days_from_civil`).
pub(super) fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Inverse of [`days_from_civil`]: `(year, month, day)` for an epoch day.
pub(super) fn civil_from_days(z: i64) -> (i64, i64, i64) {
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
pub(super) fn day_number(day: &str) -> Option<i64> {
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

/// Human duration for cell tooltips and the superlatives card: "42 m",
/// "3 h", "3 h 20 m".
pub(super) fn format_active_time(secs: i64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// Three-letter month name for a 1-based month number.
pub(super) fn month_abbr(m: i64) -> &'static str {
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

/// The live streak's inclusive day range, as epoch-day numbers.
///
/// Anchored on the **last active day**, not on `anchor`: a run that ended
/// yesterday still counts (today isn't over), so measuring back from the
/// anchor would shade one day too far and leave the outlined run disagreeing
/// with the figure reporting it. `None` when no run is live.
pub(super) fn streak_span(anchor: i64, active: &[i64], streak_days: i64) -> Option<(i64, i64)> {
    if streak_days <= 0 {
        return None;
    }
    let last = active.iter().copied().filter(|&n| n <= anchor).max()?;
    Some((last - (streak_days - 1), last))
}

/// Days with any recorded activity, and the share of the drawn window they
/// are. The denominator stops at `anchor` — a year is not yet over, and
/// counting the days after today against a reader would report every December
/// as a failure.
fn coverage(by_day: &HashMap<i64, i64>, start: i64, anchor: i64) -> (i64, i64) {
    let read = by_day
        .iter()
        .filter(|(&day, &secs)| secs > 0 && day >= start && day <= anchor)
        .count();
    let read = i64::try_from(read).unwrap_or(0);
    let elapsed = (anchor - start + 1).max(1);
    (read, read.saturating_mul(100) / elapsed)
}

/// One rendered cell: its day, seconds, and intensity bucket.
struct HeatCell {
    day: String,
    secs: i64,
    level: u8,
    /// Days after the anchor in the final week render as invisible spacers.
    future: bool,
    /// Part of the run the reader is currently on, which the grid outlines.
    in_streak: bool,
}

/// Epoch day of the grid's first cell: the Monday of `anchor`'s week, 51 weeks
/// back. Unix day 0 is a Thursday, so Monday-aligning subtracts `(n + 3) % 7`
/// — the same convention as `db::stats`' busiest-week bucketing.
///
/// Shared with the caller's `by_day` filter so the intensity scale is
/// normalized over exactly the days the grid draws — a day outside it must not
/// set the `max` every rendered cell is bucketed against.
fn grid_start(anchor: i64) -> i64 {
    let monday = anchor - (anchor + 3).rem_euclid(7);
    monday - (WEEKS - 1) * 7
}

/// Build the cell list: 52 week columns of 7 rows (Monday-first), ending in
/// the week containing `anchor`.
fn build_cells(
    anchor: i64,
    by_day: &HashMap<i64, i64>,
    max: i64,
    streak: Option<(i64, i64)>,
) -> Vec<HeatCell> {
    let start = grid_start(anchor);
    (start..start + WEEKS * 7)
        .map(|n| {
            let secs = by_day.get(&n).copied().unwrap_or(0);
            HeatCell {
                day: day_string(n),
                secs,
                level: intensity(secs, max),
                future: n > anchor,
                in_streak: secs > 0 && streak.is_some_and(|(from, to)| n >= from && n <= to),
            }
        })
        .collect()
}

/// The standing heatmap card: the trailing-year grid with the live run
/// outlined, the days-read coverage and the streak record in its header, the
/// month ruler, and the less/more legend.
///
/// The run the reader is *on* is not restated here — the hero leads with it,
/// and a second copy of the same figure two bands apart is where two surfaces
/// start to disagree. The record stands beside the coverage instead.
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
    let active: Vec<i64> = by_day
        .iter()
        .filter(|(_, &secs)| secs > 0)
        .map(|(&n, _)| n)
        .collect();
    let streak = streak_span(anchor, &active, summary.current_streak_days);
    let cells = build_cells(anchor, &by_day, max, streak);
    let months = trailing_month_labels(anchor);
    let (days_read, percent) = coverage(&by_day, window_start, anchor);
    let longest = summary.longest_streak_days;

    rsx! {
        div { class: "card st-heat", "data-testid": "stats-heatmap",
            div { class: "st-heat-head",
                div {
                    div { class: "label", "Every day of the last year" }
                    div { class: "st-heat-sub",
                        "Darker is longer. Your current run is outlined."
                    }
                }
                div { class: "st-heat-figures",
                    div { class: "st-heat-figure", "data-testid": "stats-days-read",
                        div { class: "st-heat-value", "{days_read}" }
                        div { class: "label", "days read" }
                    }
                    div { class: "st-heat-figure",
                        div { class: "st-heat-value",
                            "{percent}"
                            span { class: "st-heat-value-unit", "%" }
                        }
                        div { class: "label", "of the year" }
                    }
                    div { class: "st-heat-figure", "data-testid": "stats-longest-streak",
                        div { class: "st-heat-value accent",
                            "{longest}"
                            span { class: "st-heat-value-unit", {plural_noun(longest, " day")} }
                        }
                        // "best run", never "best runs": the figure is how
                        // long the record run was, not how many there were.
                        div { class: "label", "best run" }
                    }
                }
            }
            // Block-level scroll wrapper: iOS WebKit clips this far more reliably than a grid-as-scroller (#1076).
            div { class: "st-heatmap-scroll",
                div { class: "st-heatmap", role: "img", aria_label: "Daily reading activity, trailing year",
                    for cell in cells {
                        div {
                            key: "{cell.day}",
                            class: match (cell.future, cell.in_streak) {
                                (true, _) => "st-hm-cell st-hm-future".to_string(),
                                (false, true) => format!("st-hm-cell st-hm-{} run", cell.level),
                                (false, false) => format!("st-hm-cell st-hm-{}", cell.level),
                            },
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
                span { "less" }
                for level in 0..5 {
                    div { key: "{level}", class: "st-hm-cell st-hm-{level}" }
                }
                span { "more" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
