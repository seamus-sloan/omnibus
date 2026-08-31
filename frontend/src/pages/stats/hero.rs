//! The stats hero: the streak the reader is on, a six-week activity spark
//! beneath it, and the goal cluster beside it. Everything here is standing
//! rather than windowed — `current_streak_days`, `goal` and `daily_goals` all
//! read the same on every [`omnibus_shared::StatsRange`] — so the hero sits
//! above the period switcher entirely.

use std::collections::HashMap;

use dioxus::prelude::*;
use omnibus_shared::StatsSummary;

use super::goal::{AnnualGoalRing, DailyGoalsCard};
use super::heatmap::{day_number, streak_span};
use crate::format::plural_noun;

/// Days of history the spark draws — six weeks, the span over which a run is
/// still legible one bar per day.
const SPARK_DAYS: i64 = 42;

/// One spark bar: its day, height as a percentage of the busiest day drawn,
/// and whether it belongs to the run the reader is currently on.
struct SparkBar {
    day: String,
    height_pct: u32,
    in_streak: bool,
    active: bool,
}

/// The last [`SPARK_DAYS`] days ending at `anchor`, scaled to the busiest of
/// them. A day with no recorded activity keeps its slot — the gaps are what
/// make a run visible.
fn build_spark(summary: &StatsSummary, anchor: i64) -> Vec<SparkBar> {
    let by_day: HashMap<i64, i64> = summary
        .heatmap
        .iter()
        .filter_map(|d| day_number(&d.day).map(|n| (n, d.seconds)))
        .collect();
    let start = anchor - (SPARK_DAYS - 1);
    let max = (start..=anchor)
        .filter_map(|n| by_day.get(&n).copied())
        .max()
        .unwrap_or(0);
    let active: Vec<i64> = by_day
        .iter()
        .filter(|(_, &secs)| secs > 0)
        .map(|(&n, _)| n)
        .collect();
    let span = streak_span(anchor, &active, summary.current_streak_days);
    (start..=anchor)
        .map(|n| {
            let secs = by_day.get(&n).copied().unwrap_or(0);
            let (y, m, d) = super::heatmap::civil_from_days(n);
            SparkBar {
                day: format!("{y:04}-{m:02}-{d:02}"),
                height_pct: if max <= 0 {
                    0
                } else {
                    // `max` is the window's own maximum, so the ratio is 0..=100.
                    u32::try_from(secs.max(0) * 100 / max).unwrap_or(100)
                },
                in_streak: span.is_some_and(|(from, to)| n >= from && n <= to) && secs > 0,
                active: secs > 0,
            }
        })
        .collect()
}

/// The line under the headline: where this run stands against the record.
///
/// `None` when the reader has neither a live run nor a record — there is
/// nothing to compare, and a sentence saying so is furniture.
fn streak_line(current: i64, longest: i64) -> Option<String> {
    if longest <= 0 {
        return None;
    }
    if current >= longest {
        return Some("That is the longest run you have recorded.".to_string());
    }
    Some(format!(
        "Your longest ever is {longest} {}.",
        plural_noun(longest, "day")
    ))
}

/// The standing hero. `summary` is the all-time summary — the one fetch a
/// period switch never re-runs — and is `None` only while it is in flight.
///
/// The goals are read straight off it rather than owned as signals: nothing on
/// this page writes them any more, so there is no save to fold back in.
#[component]
pub(super) fn StatsHero(summary: Option<StatsSummary>) -> Element {
    let as_of_day = summary
        .as_ref()
        .map(|s| s.as_of_day.clone())
        .unwrap_or_default();
    // The calendar year is the server's, taken from `as_of_day` rather than a
    // client clock: the two would disagree across a New Year's Eve timezone
    // gap, and a client-derived year in markup is a hydration hazard (rule 07).
    let year = as_of_day.get(..4).unwrap_or_default().to_string();
    let current = summary.as_ref().map_or(0, |s| s.current_streak_days);
    let longest = summary.as_ref().map_or(0, |s| s.longest_streak_days);
    let goal = summary.as_ref().and_then(|s| s.goal.clone());
    let finished = summary.as_ref().and_then(|s| s.books_this_year);
    let daily = summary
        .as_ref()
        .map(|s| s.daily_goals.clone())
        .unwrap_or_default();
    let anchor = summary.as_ref().and_then(|s| day_number(&s.as_of_day));
    let spark = match (summary.as_ref(), anchor) {
        (Some(s), Some(n)) => build_spark(s, n),
        _ => Vec::new(),
    };

    rsx! {
        header { class: "st-hero", "data-testid": "stats-hero",
            div { class: "st-hero-inner",
                div { class: "st-hero-run",
                    span { class: "st-hero-kicker",
                        if current > 0 { "You are on a run" } else { "No run right now" }
                    }
                    h1 { class: "st-hero-figure", "data-testid": "stats-current-streak",
                        "{current} "
                        span { class: "st-hero-unit", {plural_noun(current, "day")} }
                        span { class: "st-hero-stop", "." }
                    }
                    // Not `stats-longest-streak` — this is the sentence
                    // *about* the record, and the heatmap's header carries
                    // the figure itself under that name.
                    if let Some(line) = streak_line(current, longest) {
                        p { class: "st-hero-line", "data-testid": "stats-streak-line", {line} }
                    }
                    if !spark.is_empty() {
                        div {
                            class: "st-spark",
                            "data-testid": "stats-spark",
                            role: "img",
                            aria_label: "Daily activity over the last six weeks",
                            for bar in spark.iter() {
                                div {
                                    key: "{bar.day}",
                                    class: match (bar.in_streak, bar.active) {
                                        (true, _) => "st-spark-bar on",
                                        (false, true) => "st-spark-bar",
                                        (false, false) => "st-spark-bar off",
                                    },
                                    style: "height: {bar.height_pct}%",
                                    title: "{bar.day}",
                                }
                            }
                        }
                        div { class: "st-spark-axis",
                            span { "6 weeks ago" }
                            span { "today" }
                        }
                    }
                }
                div { class: "st-hero-goals",
                    AnnualGoalRing { goal, finished, year, as_of_day }
                    DailyGoalsCard { daily }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
