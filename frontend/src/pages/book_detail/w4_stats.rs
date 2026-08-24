//! Stop 03 · Stats — what this read has looked like: the 2×2 record grid
//! (Started / Time in book / Pickups / Longest sit), a time-left note, the
//! per-day activity spark over the last 22 days, and the rating widget.
//! Insights arrive from the stage's shared post-mount fetch; books with no
//! sessions (and wishlist-only books) get the design's quiet empty state.

use dioxus::prelude::*;
use omnibus_shared::{BookInsights, DayActivity};

use crate::date_fmt::civil_from_days;
use crate::time::now_unix;

use super::rating::BdRatingWidget;
use super::w4::W4Progress;

/// Days shown in the activity spark, mirroring the old Insights card's strip.
const SPARK_DAYS: usize = 22;

/// The Stats stop.
#[component]
pub(super) fn W4StatsStop(
    uuid: String,
    insights: Option<BookInsights>,
    progress: W4Progress,
    audio_only: bool,
    wish_mode: bool,
) -> Element {
    rsx! {
        div { class: "bdw4-k", if wish_mode { "Stats" } else { "What this read has looked like" } }
        match insights {
            Some(i) if i.sessions > 0 && !wish_mode => rsx! {
                {render_stats(&i, &progress, audio_only)}
            },
            _ => rsx! {
                div { class: "bdw4-bigquiet", "data-testid": "bdw4-no-stats", "No stats yet." }
                p { class: "bdw4-quiet-body",
                    if wish_mode {
                        "Add an ebook or an audiobook to start tracking your reading stats for this book."
                    } else {
                        "Open the book and the record starts itself \u{2014} time, pickups, and pace all land here."
                    }
                }
            },
        }
        div { class: "bdw4-ratingrow", "data-testid": "bdw4-rating",
            BdRatingWidget { uuid: uuid.clone() }
        }
    }
}

/// The populated record: stat grid, note line, spark.
fn render_stats(i: &BookInsights, progress: &W4Progress, audio_only: bool) -> Element {
    let started_short = short_date(i.started_at);
    let days_in = ((now_unix() - i.started_at) / 86_400).max(0) + 1;
    let avg = duration_label(i.seconds_total / i.sessions.max(1));
    let time_label = if audio_only {
        "Time listened"
    } else {
        "Time in book"
    };
    let note = time_left_note(i, progress);
    let spark = spark_buckets(&i.daily, &i.as_of_day);
    let max = spark.iter().copied().max().unwrap_or(0).max(1);
    rsx! {
        div { class: "bdw4-stats", "data-testid": "bdw4-stats",
            {stat_cell("Started", &started_short, &format!("{days_in} days in"))}
            {stat_cell(time_label, &duration_label(i.seconds_total), &format!("{} sessions", i.sessions))}
            {stat_cell("Pickups", &i.sessions.to_string(), &format!("avg sit {avg}"))}
            {stat_cell("Longest sit", &duration_label(i.longest_seconds), &short_date(i.longest_started_at))}
        }
        if let Some(n) = note {
            div { class: "mono bdw4-statsnote", "{n}" }
        }
        div { class: "rx-spark", "data-testid": "bdw4-spark", aria_hidden: "true",
            for (idx, v) in spark.iter().enumerate() {
                i {
                    key: "{idx}",
                    class: if *v > 0 { "on" } else { "" },
                    style: format!("height:{}px", (2 + v * 32 / max).min(34)),
                }
            }
        }
        div { class: "rx-spark-axis",
            span { "3 wk ago" }
            span { "minutes \u{b7} by day" }
            span { "today" }
        }
    }
}

/// One `.rx-stat` cell.
fn stat_cell(k: &str, v: &str, s: &str) -> Element {
    rsx! {
        div { class: "rx-stat",
            div { class: "k", "{k}" }
            div { class: "v", "{v}" }
            div { class: "s", "{s}" }
        }
    }
}

/// "≈ Xh Ym to go at your pace" from total time and the newest percent.
fn time_left_note(i: &BookInsights, progress: &W4Progress) -> Option<String> {
    let pct = progress.newest_percent()?.clamp(0, 100);
    if pct == 0 {
        return None;
    }
    if pct >= 100 {
        return Some("finished \u{2014} the record is complete".to_string());
    }
    let left = i.seconds_total * (100 - pct) / pct;
    Some(format!(
        "\u{2248} {} to go at your pace \u{b7} {pct}% in",
        duration_label(left)
    ))
}

/// "Xh Ym" / "Xm" duration label (moved from the old Insights card).
pub(super) fn duration_label(secs: i64) -> String {
    let secs = secs.max(0);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// Short "Mon D" date from unix seconds (UTC — same bucketing as the data).
pub(super) fn short_date(unix_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = civil_from_days(unix_secs.div_euclid(86_400));
    format!("{} {d}", MONTHS[(m as usize - 1).min(11)])
}

/// The last [`SPARK_DAYS`] days of activity in minutes, oldest → newest,
/// anchored on the server's `as_of_day` so client and server agree on
/// "today". Days without sessions are zero.
fn spark_buckets(daily: &[DayActivity], as_of_day: &str) -> Vec<i64> {
    let Some(end) = parse_day(as_of_day) else {
        return vec![0; SPARK_DAYS];
    };
    let start = end - (SPARK_DAYS as i64 - 1);
    let mut out = vec![0i64; SPARK_DAYS];
    for d in daily {
        if let Some(day) = parse_day(&d.day) {
            if day >= start && day <= end {
                out[(day - start) as usize] = d.seconds / 60;
            }
        }
    }
    out
}

/// `YYYY-MM-DD` → days since the unix epoch. `None` on a malformed string.
fn parse_day(s: &str) -> Option<i64> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    Some(days_from_civil(y, m, d))
}

/// Howard Hinnant's `days_from_civil` — the inverse of the shared
/// `date_fmt::civil_from_days`: civil date → days since 1970-01-01.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m as i64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_day_roundtrips_through_civil_from_days() {
        for s in ["1970-01-01", "2000-02-29", "2023-11-14", "2026-08-24"] {
            let days = parse_day(s).unwrap();
            let (y, m, d) = civil_from_days(days);
            assert_eq!(format!("{y:04}-{m:02}-{d:02}"), s);
        }
        assert_eq!(parse_day("1970-01-01"), Some(0));
        assert_eq!(parse_day("not-a-day"), None);
    }

    #[test]
    fn spark_buckets_places_minutes_by_day_and_zero_fills_gaps() {
        let daily = vec![
            DayActivity {
                day: "2026-08-24".into(),
                seconds: 600,
            },
            DayActivity {
                day: "2026-08-03".into(),
                seconds: 120,
            },
            // Outside the 22-day window — dropped.
            DayActivity {
                day: "2026-07-01".into(),
                seconds: 999,
            },
        ];
        let out = spark_buckets(&daily, "2026-08-24");
        assert_eq!(out.len(), SPARK_DAYS);
        assert_eq!(out[SPARK_DAYS - 1], 10);
        assert_eq!(out[0], 2); // Aug 3 is exactly 21 days before Aug 24.
        assert_eq!(out.iter().sum::<i64>(), 12);
    }

    #[test]
    fn duration_label_scales_minutes_and_hours() {
        assert_eq!(duration_label(0), "0m");
        assert_eq!(duration_label(3600), "1h");
        assert_eq!(duration_label(5400), "1h 30m");
    }

    #[test]
    fn short_date_formats_utc_month_day() {
        assert_eq!(short_date(1_700_000_000), "Nov 14");
    }
}
