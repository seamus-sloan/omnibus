//! "Your reading clock" — the window's hour-of-day shape as a 24-tick radial
//! dial, with the weekday split beside it. Draws exactly the buckets the
//! server sent (`db::stats::patterns`) and re-derives nothing from a
//! timestamp, so this card and the iOS charts cannot disagree about the same
//! seconds.

use dioxus::prelude::*;
use omnibus_shared::{HourBucket, StatsSummary, WeekdayBucket};

/// Degrees per hour around the dial. Hour 0 sits at the top, which the `+180`
/// in [`tick_transform`] achieves — the tick grows *outward* from the centre,
/// so its untranslated direction is down.
const DEGREES_PER_HOUR: i64 = 15;

/// Distance from the dial's centre to the inner end of every tick, in pixels.
/// Just outside the inner ring, so the ticks read as a rim rather than as
/// spokes.
const TICK_INSET_PX: i64 = 58;

/// How far the busiest hour's tick reaches beyond [`TICK_INSET_PX`].
const TICK_REACH_PX: i64 = 40;

/// The shortest a tick is ever drawn, so a quiet-but-nonzero hour is still
/// visible as a mark on the rim.
const TICK_MIN_PX: i64 = 5;

/// One rendered tick: where it points, how far it reaches, and how loud it is.
struct Tick {
    hour: i64,
    transform: String,
    height_px: i64,
    weight: &'static str,
}

/// One weekday row: its label, bar width, colour weight, and readout.
struct Weekday {
    label: String,
    width_pct: u32,
    weight: &'static str,
    readout: String,
}

/// `rotate(...) translateY(...)` for the tick at `hour`. The rotation is
/// applied about the dial's centre — see `.st-clock-tick`'s `transform-origin`
/// in `atrium.css`, which must stay in step with this or the rim comes out
/// off-centre from the inner ring.
fn tick_transform(hour: i64) -> String {
    format!(
        "rotate({}deg) translateY({TICK_INSET_PX}px)",
        hour * DEGREES_PER_HOUR + 180
    )
}

/// Four weights, so the rim reads as a shape rather than as 24 equal marks.
/// `fraction` is the hour's share of the busiest hour, 0.0..=1.0.
fn weight_for(fraction: f64) -> &'static str {
    if fraction > 0.7 {
        "hot"
    } else if fraction > 0.35 {
        "warm"
    } else if fraction > 0.05 {
        "cool"
    } else {
        "idle"
    }
}

/// The 24 ticks, scaled to the window's busiest hour. An all-zero window draws
/// 24 minimum ticks rather than nothing — the dial's own shape says "no
/// pattern here" more clearly than an empty circle would.
fn build_ticks(hours: &[HourBucket]) -> Vec<Tick> {
    let max = hours.iter().map(|h| h.seconds).max().unwrap_or(0);
    hours
        .iter()
        .map(|h| {
            // Both non-negative and `seconds <= max`, so the ratio is 0.0..=1.0.
            #[allow(clippy::cast_precision_loss)]
            let fraction = if max > 0 {
                h.seconds.max(0) as f64 / max as f64
            } else {
                0.0
            };
            #[allow(clippy::cast_possible_truncation)]
            let reach = (fraction * TICK_REACH_PX as f64).round() as i64;
            Tick {
                hour: h.hour,
                transform: tick_transform(h.hour),
                height_px: reach.max(TICK_MIN_PX),
                weight: weight_for(fraction),
            }
        })
        .collect()
}

/// "8pm" / "12am" — the hour in the clock a reader keeps, not a 24-hour index.
fn hour_label(hour: i64) -> String {
    let h = hour.rem_euclid(24);
    let display = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{display}{}", if h < 12 { "am" } else { "pm" })
}

/// The busiest hour's label, or the em-dash when the window has no placeable
/// activity at all.
///
/// Derived rather than asserted: an all-zero strip has no peak, and taking the
/// first maximum of a row of zeros would claim the reader reads at midnight.
fn peak_hour(hours: &[HourBucket]) -> String {
    let peak = hours
        .iter()
        .filter(|h| h.seconds > 0)
        .max_by_key(|h| h.seconds);
    peak.map_or_else(|| "\u{2014}".to_string(), |h| hour_label(h.hour))
}

/// The four parts of a day the clock line names, each with the copy that
/// introduces it and the span it covers.
const BANDS: [(&str, &str, i64, i64); 4] = [
    ("You read early", "before noon", 5, 11),
    ("Afternoons are yours", "between noon and 5pm", 12, 16),
    ("Evenings are yours", "between 5pm and 10pm", 17, 21),
    ("You read late", "after 10pm", 22, 4),
];

/// Seconds recorded inside a band, which may wrap past midnight.
fn band_seconds(hours: &[HourBucket], from: i64, to: i64) -> i64 {
    hours
        .iter()
        .filter(|h| {
            if from <= to {
                h.hour >= from && h.hour <= to
            } else {
                h.hour >= from || h.hour <= to
            }
        })
        .map(|h| h.seconds)
        .sum()
}

/// The sentence beside the dial: which part of the day the window belongs to,
/// and how much of it lands there.
///
/// `None` when nothing in the window can be placed on a local clock — the
/// card renders its empty state instead of a sentence about zero.
fn clock_line(hours: &[HourBucket]) -> Option<String> {
    let total: i64 = hours.iter().map(|h| h.seconds).sum();
    if total <= 0 {
        return None;
    }
    let (lead, span, seconds) = BANDS
        .iter()
        .map(|(lead, span, from, to)| (lead, span, band_seconds(hours, *from, *to)))
        .max_by_key(|(_, _, seconds)| *seconds)?;
    let pct = seconds.saturating_mul(100) / total;
    Some(format!(
        "{lead} \u{2014} {pct}% of your recorded time lands {span}."
    ))
}

/// "58m" / "1h 28m" / "3h" — the magnitude a reader can't take off a bar, and
/// an em-dash for a day with nothing on it.
///
/// A quiet day reads as an absence, never as "0h 0m": a weekday strip is
/// zero-filled to seven columns, so a day nobody read has to look different
/// from a day that was measured at zero.
fn day_readout(seconds: i64) -> String {
    if seconds <= 0 {
        return "\u{2014}".to_string();
    }
    let (h, m) = (seconds / 3600, (seconds % 3600) / 60);
    match (h, m) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// The seven weekday rows, scaled to the busiest day in the window.
fn build_weekdays(days: &[WeekdayBucket]) -> Vec<Weekday> {
    let max = days.iter().map(|d| d.seconds).max().unwrap_or(0);
    days.iter()
        .map(|d| Weekday {
            // Server-sent, never derived here: week-start is a convention, and
            // a client that assumed Sunday-first would draw every row one
            // place out.
            label: d.label.clone(),
            width_pct: if max > 0 {
                u32::try_from(d.seconds.max(0) * 100 / max).unwrap_or(100)
            } else {
                0
            },
            weight: if d.seconds <= 0 {
                "idle"
            } else if d.seconds == max {
                "hot"
            } else {
                "warm"
            },
            readout: day_readout(d.seconds),
        })
        .collect()
}

/// The disclosure line for activity that carries no capture-time timezone —
/// sessions recorded before the offset was captured, so the dial above is
/// drawn over less than the window's whole total. Stated rather than
/// absorbed: bucketing those seconds in UTC would put a reader's evening at
/// 4am.
fn unzoned_note(seconds: i64) -> Option<String> {
    (seconds > 0).then(|| {
        format!(
            "{} of activity was recorded without a timezone and isn\u{2019}t shown here.",
            day_readout(seconds)
        )
    })
}

/// The reading-clock card: the dial, the sentence, and the weekday split.
#[component]
pub(super) fn ReadingClock(summary: StatsSummary) -> Element {
    let note = unzoned_note(summary.unzoned_seconds);
    if !summary.has_time_patterns() {
        return rsx! {
            div { class: "card st-clock", "data-testid": "stats-when",
                div { class: "label", "Your reading clock" }
                p { class: "st-card-empty", "data-testid": "stats-when-empty",
                    "No activity with a recorded local time in this period yet."
                }
                if let Some(note) = note {
                    p { class: "st-card-note", "data-testid": "stats-when-unzoned", "{note}" }
                }
            }
        };
    }
    let ticks = build_ticks(&summary.hour_of_day);
    let weekdays = build_weekdays(&summary.day_of_week);
    let peak = peak_hour(&summary.hour_of_day);
    let line = clock_line(&summary.hour_of_day);

    rsx! {
        div { class: "card st-clock", "data-testid": "stats-when",
            div { class: "label", "Your reading clock" }
            div { class: "st-clock-body",
                div {
                    class: "st-clock-dial",
                    "data-testid": "stats-clock-dial",
                    role: "img",
                    aria_label: "Activity by hour of day",
                    for tick in ticks.iter() {
                        div {
                            key: "{tick.hour}",
                            class: "st-clock-tick {tick.weight}",
                            style: "transform: {tick.transform}; height: {tick.height_px}px",
                        }
                    }
                    div { class: "st-clock-hub",
                        div { class: "st-clock-peak", "data-testid": "stats-clock-peak", {peak} }
                        div { class: "st-clock-peak-label", "peak hour" }
                    }
                }
                div { class: "st-clock-side",
                    if let Some(line) = line {
                        p { class: "st-clock-line", "data-testid": "stats-clock-line", {line} }
                    }
                    div { class: "st-days", "data-testid": "stats-when-weekdays",
                        for day in weekdays.iter() {
                            div { key: "{day.label}", class: "st-day",
                                span { class: "st-day-label", "{day.label}" }
                                div { class: "st-day-track",
                                    div {
                                        class: "st-day-bar {day.weight}",
                                        style: "width: {day.width_pct}%",
                                    }
                                }
                                span { class: "st-day-readout", "{day.readout}" }
                            }
                        }
                    }
                }
            }
            if let Some(note) = note {
                p { class: "st-card-note", "data-testid": "stats-when-unzoned", "{note}" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
