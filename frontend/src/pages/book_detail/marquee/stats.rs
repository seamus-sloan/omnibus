//! Stop 03 · Stats — what this read has looked like: the 2×2 record grid
//! (Started / Time in book / Pickups / Longest sit), a time-left note, the
//! per-day activity spark over the last 22 days, the session log behind those
//! figures, and the rating widget.
//! Insights arrive from the stage's shared post-mount fetch; books with no
//! sessions (and wishlist-only books) get the design's quiet empty state.

use dioxus::prelude::*;
use omnibus_shared::{BookInsights, DayActivity};

// The log renders directly beneath this grid, so a sitting's length and
// the "Longest sit" above it must be spelled the same way.
pub(super) use crate::components::session_log::duration_label;
use crate::components::SessionLogList;
use crate::date_fmt::civil_from_days;
use crate::format::count_label;
use crate::time::now_unix;
use crate::time::{local_date_offset, use_local_dates_ready};

use crate::pages::book_detail::rating::BdRatingWidget;

use super::MarqueeProgress;

/// Days shown in the activity spark, mirroring the old Insights card's strip.
const SPARK_DAYS: usize = 22;

/// The Stats stop.
#[component]
pub(super) fn MarqueeStatsStop(
    uuid: String,
    insights: Option<BookInsights>,
    progress: MarqueeProgress,
    audio_only: bool,
    wish_mode: bool,
) -> Element {
    // Hoisted for the whole stop: every date below is a stored instant, and
    // they must all land on the reader's calendar together (#2464).
    let dates_ready = use_local_dates_ready()();
    rsx! {
        div { class: "bdmq-k", if wish_mode { "Stats" } else { "What this read has looked like" } }
        match insights {
            Some(i) if i.sessions > 0 && !wish_mode => rsx! {
                {render_stats(&i, &progress, audio_only, dates_ready)}
                div { class: "bdmq-k bdmq-logk", "The sittings behind it" }
                SessionLogList { book: Some(uuid.clone()), compact: true }
            },
            _ => rsx! {
                div { class: "bdmq-bigquiet", "data-testid": "bdmq-no-stats", "No stats yet." }
                p { class: "bdmq-quiet-body",
                    if wish_mode {
                        "Add an ebook or an audiobook to start tracking your reading stats for this book."
                    } else {
                        "Open the book and the record starts itself \u{2014} time, pickups, and pace all land here."
                    }
                }
            },
        }
        div { class: "bdmq-ratingrow", "data-testid": "bdmq-rating",
            BdRatingWidget { uuid: uuid.clone() }
        }
    }
}

/// The populated record: stat grid, note line, spark.
fn render_stats(
    i: &BookInsights,
    progress: &MarqueeProgress,
    audio_only: bool,
    dates_ready: bool,
) -> Element {
    let started_short = short_date(i.started_at, local_date_offset(dates_ready, i.started_at));
    let started_sub = days_in_label(now_unix(), i.started_at);
    let avg = duration_label(avg_sit_secs(i));
    let time_label = if audio_only {
        "Time listened"
    } else {
        "Time in book"
    };
    let note = time_left_note(i, progress);
    let spark = spark_buckets(&i.daily, &i.as_of_day);
    let max = spark.iter().copied().max().unwrap_or(0).max(1);
    rsx! {
        div { class: "bdmq-stats", "data-testid": "bdmq-stats",
            {stat_cell("Started", &started_short, &started_sub)}
            {stat_cell(time_label, &duration_label(i.seconds_total), &count_label(i.sessions, "session"))}
            {stat_cell("Pickups", &i.sessions.to_string(), &format!("avg sit {avg}"))}
            {stat_cell(
                "Longest sit",
                &duration_label(i.longest_seconds),
                &short_date(
                    i.longest_started_at,
                    local_date_offset(dates_ready, i.longest_started_at),
                ),
            )}
        }
        if let Some(n) = note {
            div { class: "mono bdmq-statsnote", "{n}" }
        }
        // One labelled image, not 22 bars announced a tick at a time. The role
        // also makes it a leaf, so the ticks need no hiding of their own.
        div {
            class: "rx-spark",
            "data-testid": "bdmq-spark",
            role: "img",
            "aria-label": "{spark_summary(&spark, &i.as_of_day, audio_only)}",
            for (idx, v) in spark.iter().enumerate() {
                i {
                    key: "{idx}",
                    class: if *v > 0 { "on" } else { "" },
                    style: format!("height:{}px", (2 + v * 32 / max).min(34)),
                }
            }
        }
        // The bars' visual scale; the label above already states window and unit.
        div { class: "rx-spark-axis", aria_hidden: "true",
            span { "3 wk ago" }
            span { "minutes \u{b7} by day" }
            span { "today" }
        }
    }
}

/// The "Started" tile's sub-label: how long the book has been in progress,
/// counting *elapsed* days. A book started within the last day reads "today"
/// rather than "1 day in" — the old inclusive `+ 1` count printed "1 day in"
/// the moment a book was opened, which reads as an off-by-one (#2357). A full
/// day or more elapsed reads "N day(s) in". `now` is injected so the label is
/// testable on a fixed clock.
fn days_in_label(now: i64, started_at: i64) -> String {
    let days = ((now - started_at) / 86_400).max(0);
    if days == 0 {
        "today".to_string()
    } else {
        format!("{} in", count_label(days, "day"))
    }
}

/// Mean seconds per counted sitting, the "avg sit" line under Pickups.
///
/// Divides `sitting_seconds` rather than `seconds_total`: the latter also
/// carries glances too short to be sittings, which would push the mean above
/// the "Longest sit" rendered in the next cell.
fn avg_sit_secs(i: &BookInsights) -> i64 {
    i.sitting_seconds / i.sessions.max(1)
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

/// Smallest percent that yields a usable pace estimate. Below it the
/// extrapolation multiplies elapsed time by more than nineteen, which is a
/// guess wearing the same clothes as a measurement. The note is dropped
/// instead: a missing estimate reads as "not yet", a wrong one reads as fact.
const MIN_PACE_PERCENT: i64 = 5;

/// "≈ Xh Ym to go at your pace" from total time and the newest percent.
/// `None` below [`MIN_PACE_PERCENT`], where the extrapolation isn't meaningful.
fn time_left_note(i: &BookInsights, progress: &MarqueeProgress) -> Option<String> {
    let pct = progress.newest_percent()?.clamp(0, 100);
    if pct >= 100 {
        return Some("finished \u{2014} the record is complete".to_string());
    }
    if pct < MIN_PACE_PERCENT {
        return None;
    }
    let left = i.seconds_total * (100 - pct) / pct;
    Some(format!(
        "\u{2248} {} to go at your pace \u{b7} {pct}% in",
        duration_label(left)
    ))
}

/// Short "Mon D" date from unix seconds, shifted by `offset_secs`.
///
/// **Pass an offset only for a real instant.** A raw `started_at` is one, and
/// dating it in UTC put a 23:36 sitting on the following day while the spark
/// beside it — bucketed on the reader's own calendar by the server — drew it
/// on the right one (#2464). A value already derived from a *local day
/// number* (`day * 86_400`) is midnight of a day that has been placed
/// already; shifting it again moves it off that day.
pub(super) fn short_date(unix_secs: i64, offset_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (_, m, d) = civil_from_days((unix_secs + offset_secs).div_euclid(86_400));
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

/// The spark's text alternative: what the bars say, in a sentence.
///
/// Totals and a peak rather than 22 readings — the picture's job is the shape
/// of a reading habit, and "143 minutes across 6 days, most on Aug 24" carries
/// that where an announced list of numbers would not.
///
/// Ties on the peak break to the **earliest** day, matching how
/// `db::stats::book_insights` picks the longest sitting, so the two figures on
/// one card can't name different days for the same data.
///
/// `audio_only` picks the verb off the same flag the "Time listened" tile
/// above reads: the buckets union `reading_sessions` and `listening_sessions`,
/// so a book with only an audiobook is minutes *listened*, and calling them
/// minutes read is the one thing this sentence can state that is false.
fn spark_summary(spark: &[i64], as_of_day: &str, audio_only: bool) -> String {
    let verb = if audio_only { "listened" } else { "read" };
    let window = count_label(spark.len() as i64, "day");
    let total: i64 = spark.iter().sum();
    if total == 0 {
        return format!("Minutes {verb} per day over the last {window}: none.");
    }
    let active = count_label(spark.iter().filter(|v| **v > 0).count() as i64, "day");
    // Strict `>` keeps the first of equal peaks.
    let (peak_idx, peak) =
        spark.iter().enumerate().fold(
            (0usize, 0i64),
            |best, (idx, &v)| {
                if v > best.1 {
                    (idx, v)
                } else {
                    best
                }
            },
        );
    let summary = format!(
        "Minutes {verb} per day over the last {window}: {} across {active}",
        count_label(total, "minute")
    );
    // The anchor is the same one the buckets were laid out against, so the
    // named day is the bar a sighted reader sees at that position. It only
    // fails to parse when every bucket is zero, which returned above.
    let Some(end) = parse_day(as_of_day) else {
        return format!("{summary}.");
    };
    let day = end - (spark.len() as i64 - 1) + peak_idx as i64;
    format!(
        "{summary}, most on {} with {}.",
        // Already a local day number from the server; see `short_date`.
        short_date(day * 86_400, 0),
        count_label(peak, "minute")
    )
}

/// `YYYY-MM-DD` → days since the unix epoch. `None` on a malformed string or
/// an impossible calendar date (the round-trip through `civil_from_days`
/// rejects e.g. `2026-02-31`, which `days_from_civil` would silently shift).
fn parse_day(s: &str) -> Option<i64> {
    let mut parts = s.splitn(3, '-');
    let y: i64 = parts.next()?.parse().ok()?;
    let m: u32 = parts.next()?.parse().ok()?;
    let d: u32 = parts.next()?.parse().ok()?;
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let days = days_from_civil(y, m, d);
    (civil_from_days(days) == (y, m, d)).then_some(days)
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
        // Impossible calendar dates are rejected, not silently shifted.
        assert_eq!(parse_day("2026-02-31"), None);
        assert_eq!(parse_day("2025-02-29"), None);
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
    fn spark_summary_states_the_window_total_active_days_and_peak() {
        let mut spark = vec![0i64; SPARK_DAYS];
        spark[SPARK_DAYS - 1] = 10;
        spark[0] = 2;
        assert_eq!(
            spark_summary(&spark, "2026-08-24", false),
            "Minutes read per day over the last 22 days: 12 minutes across 2 days, \
             most on Aug 24 with 10 minutes."
        );
    }

    #[test]
    fn spark_summary_says_none_rather_than_naming_a_peak_of_zero() {
        // A book opened once outside the window has an all-zero strip. Naming
        // a "busiest day" there would invent a reading day that never was.
        let summary = spark_summary(&[0; SPARK_DAYS], "2026-08-24", false);
        assert_eq!(summary, "Minutes read per day over the last 22 days: none.");
        assert!(!summary.contains("most on"));
    }

    #[test]
    fn spark_summary_says_listened_for_an_audio_only_book() {
        // The buckets union reading *and* listening sessions, so an
        // audiobook's minutes were never read — and the tile directly above
        // this strip already says "Time listened", so the two must agree.
        let mut spark = vec![0i64; SPARK_DAYS];
        spark[SPARK_DAYS - 1] = 30;
        let audio = spark_summary(&spark, "2026-08-24", true);
        assert!(audio.starts_with("Minutes listened per day"), "{audio}");
        assert!(!audio.contains("read"), "{audio}");
        // Including when there is nothing in the window to describe.
        assert_eq!(
            spark_summary(&[0; SPARK_DAYS], "2026-08-24", true),
            "Minutes listened per day over the last 22 days: none."
        );
    }

    #[test]
    fn spark_summary_names_the_earliest_of_equal_peaks() {
        // Same tie-break as `book_insights`'s longest sitting, so the two
        // figures on one card can't name different days for the same data.
        let mut spark = vec![0i64; SPARK_DAYS];
        spark[SPARK_DAYS - 3] = 7;
        spark[SPARK_DAYS - 1] = 7;
        assert!(
            spark_summary(&spark, "2026-08-24", false).contains("most on Aug 22 with 7 minutes")
        );
    }

    #[test]
    fn spark_summary_singularizes_a_lone_day_and_minute() {
        let mut spark = vec![0i64; SPARK_DAYS];
        spark[SPARK_DAYS - 1] = 1;
        assert!(spark_summary(&spark, "2026-08-24", false).contains("1 minute across 1 day,"));
    }

    #[test]
    fn spark_summary_drops_the_peak_day_when_the_anchor_is_unparsable() {
        // `spark_buckets` zero-fills on a bad anchor, so this pairing can't
        // arise from the real fetch — pinned so the fallback stays a dropped
        // clause rather than a panic or a day counted from the epoch.
        let mut spark = vec![0i64; SPARK_DAYS];
        spark[3] = 5;
        let summary = spark_summary(&spark, "not-a-day", false);
        assert_eq!(
            summary,
            "Minutes read per day over the last 22 days: 5 minutes across 1 day."
        );
    }

    #[test]
    fn duration_label_scales_minutes_and_hours() {
        assert_eq!(duration_label(0), "0m");
        assert_eq!(duration_label(59), "0m");
        assert_eq!(duration_label(60), "1m");
        assert_eq!(duration_label(3600), "1h");
        assert_eq!(duration_label(5400), "1h 30m");
        assert_eq!(duration_label(7 * 3600 + 5 * 60), "7h 5m");
    }

    #[test]
    fn duration_label_clamps_negative_input_to_zero() {
        assert_eq!(duration_label(-100), "0m");
    }

    pub(super) fn insights(
        seconds_total: i64,
        sessions: i64,
        sitting_seconds: i64,
        longest: i64,
    ) -> BookInsights {
        BookInsights {
            started_at: 0,
            seconds_total,
            sessions,
            sitting_seconds,
            longest_seconds: longest,
            longest_started_at: 0,
            daily: vec![],
            as_of_day: "2026-08-27".into(),
        }
    }

    #[test]
    fn avg_sit_never_exceeds_the_longest_sit_beside_it() {
        // One 30m sitting plus 40 glances of 30s: `seconds_total` carries all
        // 3000s, but only the 1800s sitting was counted, so the mean must be
        // 30m — not the 50m that dividing the full total would print next to
        // a "Longest sit" of 30m.
        let i = insights(3_000, 1, 1_800, 1_800);
        assert_eq!(avg_sit_secs(&i), 1_800);
        assert!(avg_sit_secs(&i) <= i.longest_seconds);
    }

    #[test]
    fn avg_sit_means_the_counted_sittings() {
        let i = insights(3_700, 2, 3_600, 2_400);
        assert_eq!(avg_sit_secs(&i), 1_800);
    }

    #[test]
    fn avg_sit_does_not_divide_by_zero_without_sittings() {
        assert_eq!(avg_sit_secs(&insights(40, 0, 0, 0)), 0);
    }

    /// A `MarqueeProgress` carrying one reading position at `pct`.
    pub(super) fn progress_at(pct: i64) -> MarqueeProgress {
        MarqueeProgress {
            reading: Some(omnibus_shared::ProgressRecord {
                book_uuid: "uuid-1".into(),
                format: omnibus_shared::ProgressFormat::Epub,
                epub_cfi: None,
                audio_position_seconds: None,
                progress_percent: Some(pct),
                kobo_location: None,
                book_file_id: None,
                updated_at: 0,
                client_updated_at: 0,
                total_duration_seconds: None,
                resolved: None,
            }),
            listening: None,
        }
    }

    #[test]
    fn time_left_note_is_withheld_below_the_pace_threshold() {
        // One hour in at 1% would extrapolate to "99h to go" — a number with
        // no basis, printed with the same confidence as a real estimate.
        let i = insights(3_600, 1, 3_600, 3_600);
        // 0 included: the explicit zero guard is gone, so the threshold is
        // the only thing between the caller and a divide by zero.
        for pct in [0, 1, 2, MIN_PACE_PERCENT - 1] {
            assert_eq!(time_left_note(&i, &progress_at(pct)), None, "pct {pct}");
        }
    }

    #[test]
    fn time_left_note_estimates_from_the_threshold_upward() {
        // 1h at 50% → 1h to go.
        let i = insights(3_600, 1, 3_600, 3_600);
        let note = time_left_note(&i, &progress_at(50)).unwrap();
        assert!(note.contains("1h"), "unexpected note: {note}");
        assert!(note.contains("50% in"), "unexpected note: {note}");
        assert!(time_left_note(&i, &progress_at(MIN_PACE_PERCENT)).is_some());
    }

    #[test]
    fn time_left_note_reports_completion_at_full_progress() {
        let i = insights(3_600, 1, 3_600, 3_600);
        assert_eq!(
            time_left_note(&i, &progress_at(100)).as_deref(),
            Some("finished \u{2014} the record is complete")
        );
    }

    #[test]
    fn short_date_formats_the_month_day_on_the_given_offset() {
        assert_eq!(short_date(1_700_000_000, 0), "Nov 14");
        // A 23:13 local sitting west of UTC stays on its own day (#2464).
        assert_eq!(short_date(1_700_000_000, -5 * 3600), "Nov 14");
        assert_eq!(short_date(1_700_006_400, -5 * 3600), "Nov 14");
    }

    #[test]
    fn days_in_label_reads_today_on_the_first_day_then_counts_elapsed_days() {
        let now = 10 * 86_400;
        // Same day (started an hour ago) and a future timestamp from clock
        // skew both read "today" (#2357) — never a countdown, never "1 day in".
        assert_eq!(days_in_label(now, now - 3_600), "today");
        assert_eq!(days_in_label(now, now + 100), "today");
        // A full elapsed day is "1 day in" (singular), more are pluralized.
        assert_eq!(days_in_label(now, now - 86_400), "1 day in");
        assert_eq!(days_in_label(now, now - 3 * 86_400), "3 days in");
    }
}

// Render-smoke coverage of the stat grid's counted nouns — a separate module
// because SSR (`dioxus::ssr`) needs the `server` feature.
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::tests::{insights, progress_at};
    use super::*;
    use crate::test_support::render;

    // Regression for issue #2250: a count of one renders a singular noun.
    // One full elapsed day reads "1 day in" (singular), never "1 days in".
    #[test]
    fn stat_grid_renders_singular_nouns_for_a_count_of_one() {
        let mut i = insights(3_600, 1, 3_600, 3_600);
        i.started_at = now_unix() - 86_400;
        let html = render(render_stats(&i, &progress_at(50), false, false));
        assert!(html.contains("1 day in"), "{html}");
        assert!(html.contains("1 session"), "{html}");
        assert!(!html.contains("1 days in"), "{html}");
        assert!(!html.contains("1 sessions"), "{html}");
    }

    #[test]
    fn stat_grid_keeps_plural_nouns_above_one() {
        let mut i = insights(3_600, 4, 3_600, 3_600);
        i.started_at = now_unix() - 3 * 86_400;
        let html = render(render_stats(&i, &progress_at(50), false, false));
        assert!(html.contains("3 days in"), "{html}");
        assert!(html.contains("4 sessions"), "{html}");
    }

    // #2357: the Started sub-label reads "today", not the old "1 day in". The
    // spark axis also carries a "today" tick, so a book started today makes it
    // the second occurrence — one alone would mean the Started cell said
    // something else.
    #[test]
    fn stat_grid_reads_today_on_the_start_day() {
        let mut i = insights(3_600, 1, 3_600, 3_600);
        i.started_at = now_unix();
        let html = render(render_stats(&i, &progress_at(50), false, false));
        assert_eq!(html.matches("today").count(), 2, "{html}");
        assert!(!html.contains("day in"), "{html}");
    }

    // The strip carries information, so it is a labelled image rather than
    // markup hidden from assistive tech — and the label is the data, not a
    // restatement of the heading above it.
    #[test]
    fn spark_is_a_labelled_image_rather_than_hidden_from_assistive_tech() {
        let mut i = insights(3_600, 1, 3_600, 3_600);
        i.daily = vec![DayActivity {
            day: "2026-08-27".into(),
            seconds: 1_800,
        }];
        let html = render(render_stats(&i, &progress_at(50), false, false));
        assert!(html.contains("role=\"img\""), "{html}");
        assert!(
            html.contains("30 minutes across 1 day, most on Aug 27 with 30 minutes"),
            "{html}"
        );
    }

    // The `audio_only` flag has to reach the label, not just exist: the tile
    // and the strip are drawn from one call, so an audiobook that reports
    // "Time listened" above minutes "read" below is one wire away.
    #[test]
    fn spark_label_speaks_the_same_format_as_the_tile_above_it() {
        let mut i = insights(3_600, 1, 3_600, 3_600);
        i.daily = vec![DayActivity {
            day: "2026-08-27".into(),
            seconds: 1_800,
        }];
        let html = render(render_stats(&i, &progress_at(50), true, false));
        assert!(html.contains("Time listened"), "{html}");
        assert!(html.contains("Minutes listened per day"), "{html}");
        assert!(!html.contains("Minutes read per day"), "{html}");
    }

    // The axis is the picture's scale, so it stays hidden — but the strip it
    // labels must not be. Asserted together because the fix for one is exactly
    // what could reintroduce the other.
    #[test]
    fn spark_axis_stays_hidden_while_the_strip_it_labels_does_not() {
        let i = insights(3_600, 1, 3_600, 3_600);
        let html = render(render_stats(&i, &progress_at(50), false, false));
        // Exactly one, and it is the axis's: the axis renders *after* the
        // strip, so nothing up to the strip's own testid may carry one.
        assert_eq!(html.matches("aria-hidden=\"true\"").count(), 1, "{html}");
        let (before_spark, _) = html
            .split_once("data-testid=\"bdmq-spark\"")
            .expect("the strip renders with its testid");
        assert!(!before_spark.contains("aria-hidden"), "{html}");
        assert!(
            html.contains("class=\"rx-spark-axis\" aria-hidden=\"true\""),
            "{html}"
        );
    }
}
