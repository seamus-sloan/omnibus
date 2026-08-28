//! Period-scoped time-pattern card for `super::StatsPage`: a 24-column
//! hour-of-day strip and a 7-column day-of-week strip on the drill-in trend's
//! pure-CSS normalized-bar treatment. Renders exactly the buckets the server
//! sent and re-derives nothing from a timestamp, so this card and the iOS
//! Charts sections cannot disagree — see `db::stats::patterns`.

use dioxus::prelude::*;
use omnibus_shared::{HourBucket, StatsSummary, WeekdayBucket};

/// Hours labelled on the 24-column axis. Every third one: labelling all 24
/// crowds them into a smear on a phone, and the unlabelled columns still read
/// against the ticks either side of them.
const HOUR_LABEL_STEP: i64 = 3;

/// One rendered column: its axis label (blank where the axis is unlabelled),
/// the hover text, and a height 0..=100 relative to the strip's tallest bar.
struct Column {
    key: String,
    label: String,
    title: String,
    height_pct: u32,
}

/// Scale a series to 0..=100 of its own maximum. An all-zero series stays all
/// zero rather than normalizing to a flat full-height row.
fn normalize(values: &[i64]) -> Vec<u32> {
    let max = values.iter().copied().max().unwrap_or(0);
    values
        .iter()
        .map(|&v| {
            if max <= 0 {
                return 0;
            }
            // Both operands are non-negative and `v <= max`, so the ratio is
            // 0..=1 and the scaled value 0..=100.
            let pct = v.max(0).saturating_mul(100) / max;
            u32::try_from(pct).unwrap_or(100)
        })
        .collect()
}

/// "4h 12m" / "1h" / "35m" / "50s" — the magnitude a reader can't take off a
/// bar.
///
/// Kept character-for-character in step with iOS `Format.humanDuration`: this
/// is the one string both surfaces build from the same wire number, so a
/// divergence here reads as the two charts disagreeing about the same
/// seconds.
fn duration_label(seconds: i64) -> String {
    if seconds <= 0 {
        return "0m".to_string();
    }
    let (h, m) = (seconds / 3600, (seconds % 3600) / 60);
    if h > 0 {
        return if m > 0 {
            format!("{h}h {m}m")
        } else {
            format!("{h}h")
        };
    }
    if m > 0 {
        return format!("{m}m");
    }
    format!("{seconds}s")
}

/// `21:00` — the hour column's hover title reads as a clock time, since a
/// bare "21" beside a duration is ambiguous.
fn hour_title(hour: i64, seconds: i64) -> String {
    format!("{hour:02}:00 \u{00B7} {}", duration_label(seconds))
}

fn hour_columns(buckets: &[HourBucket]) -> Vec<Column> {
    let heights = normalize(&buckets.iter().map(|b| b.seconds).collect::<Vec<_>>());
    buckets
        .iter()
        .zip(heights)
        .map(|(b, height_pct)| Column {
            key: format!("h{}", b.hour),
            label: if b.hour % HOUR_LABEL_STEP == 0 {
                format!("{:02}", b.hour)
            } else {
                String::new()
            },
            title: hour_title(b.hour, b.seconds),
            height_pct,
        })
        .collect()
}

fn weekday_columns(buckets: &[WeekdayBucket]) -> Vec<Column> {
    let heights = normalize(&buckets.iter().map(|b| b.seconds).collect::<Vec<_>>());
    buckets
        .iter()
        .zip(heights)
        .map(|(b, height_pct)| Column {
            key: format!("d{}", b.weekday),
            // Server-sent, never derived here: week-start is a convention, and
            // a client that assumed Sunday-first would silently draw every
            // column one place out.
            label: b.label.clone(),
            title: format!("{} \u{00B7} {}", b.label, duration_label(b.seconds)),
            height_pct,
        })
        .collect()
}

/// The disclosure line for activity that carries no capture-time timezone —
/// sessions recorded before the offset was captured, so the strips above are
/// drawn over less than the period's whole total. Stated rather than absorbed:
/// bucketing those seconds in UTC would put a reader's evening at 4am.
fn unzoned_note(seconds: i64) -> Option<String> {
    (seconds > 0).then(|| {
        format!(
            "{} of activity was recorded without a timezone and isn\u{2019}t shown here.",
            duration_label(seconds)
        )
    })
}

/// "When you read" — the hour-of-day and day-of-week strips for the period.
///
/// Renders its empty state when no session in the window can be placed on a
/// local clock, rather than two rows of flat zeros: a fixed-width strip has
/// the same shape empty as it does full, so "no data" has to say so.
#[component]
pub(super) fn TimePatternsCard(summary: StatsSummary) -> Element {
    let note = unzoned_note(summary.unzoned_seconds);
    if !summary.has_time_patterns() {
        return rsx! {
            div { class: "card st-when-card", "data-testid": "stats-when",
                div { class: "label", "When you read" }
                p { class: "st-donut-empty", "data-testid": "stats-when-empty",
                    "No activity with a recorded local time in this period yet."
                }
                if let Some(note) = note {
                    p { class: "st-when-note", "data-testid": "stats-when-unzoned", "{note}" }
                }
            }
        };
    }
    rsx! {
        div { class: "card st-when-card", "data-testid": "stats-when",
            div { class: "label", "When you read" }
            div { class: "st-when-strips",
                div { class: "st-when-strip",
                    div { class: "mono st-when-strip-label", "Hour of day" }
                    {strip(&hour_columns(&summary.hour_of_day), "stats-when-hours", "Activity by hour of day")}
                }
                div { class: "st-when-strip",
                    div { class: "mono st-when-strip-label", "Day of week" }
                    {strip(&weekday_columns(&summary.day_of_week), "stats-when-weekdays", "Activity by day of week")}
                }
            }
            if let Some(note) = note {
                p { class: "st-when-note", "data-testid": "stats-when-unzoned", "{note}" }
            }
        }
    }
}

/// One normalized-column strip. Same shape as the drill-in's trend bars, but
/// with its own class so a 24-column strip can size its columns tighter than a
/// 12-point trend chart without either having to compromise.
fn strip(columns: &[Column], testid: &str, aria_label: &str) -> Element {
    rsx! {
        div {
            class: "st-when-cols",
            "data-testid": "{testid}",
            role: "img",
            aria_label: "{aria_label}",
            for col in columns.iter() {
                div { key: "{col.key}", class: "st-when-col", title: "{col.title}",
                    div { class: "st-when-track",
                        div { class: "st-when-bar", style: "height: {col.height_pct}%;" }
                    }
                    div {
                        class: "st-when-label mono",
                        "data-testid": "stats-when-col-label",
                        "{col.label}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
