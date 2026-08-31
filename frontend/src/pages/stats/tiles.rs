//! The windowed band's headline tiles: Finished, Pages read, Listening, and
//! How you rated. Every figure here is period-scoped, so each carries its own
//! comparison against the same slice of the previous window — the delta above
//! the bar, and the bar as its shape.

use dioxus::prelude::*;
use omnibus_shared::{PeriodComparison, StatsRange, StatsSummary};

use super::drill_in::Metric;
use super::group_thousands;

/// How a tile's figure compares with the same slice of the previous window:
/// the signed label, and the fill 0..=100 the bar draws.
///
/// `None` on [`StatsRange::AllTime`], which has no previous window —
/// `PeriodComparison` is `Default` there, so a delta drawn against it would
/// report every lifetime figure as brand new. The whole comparison row is
/// dropped rather than shown against a zero nobody measured.
struct Comparison {
    label: String,
    css_class: &'static str,
    fill_pct: u32,
}

/// Bar fill for a figure against its baseline: how close this window has come
/// to the last one, pegged once it is past. A window with no baseline and
/// something to show fills; one with nothing to show stays empty.
fn fill_pct(current: f64, previous: f64) -> u32 {
    if previous <= 0.0 {
        return u32::from(current > 0.0) * 100;
    }
    // Ratio of two non-negative display figures, clamped before the cast.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let pct = (current / previous * 100.0).clamp(0.0, 100.0).round() as u32;
    pct
}

/// Direction class for a signed change, so the tile can tint an improvement
/// without the caller restating the rule per metric.
fn direction(change: f64) -> &'static str {
    if change > 0.0 {
        "up"
    } else if change < 0.0 {
        "down"
    } else {
        "flat"
    }
}

/// A count delta stated in whole units — "+2", "−1", "flat". The right form
/// for books finished, where a percentage over a handful of books is noise.
fn count_comparison(current: i64, previous: i64) -> Comparison {
    let change = current - previous;
    #[allow(clippy::cast_precision_loss)]
    let (cur, prev) = (current as f64, previous as f64);
    Comparison {
        label: if change == 0 {
            "flat".to_string()
        } else {
            format!(
                "{}{}",
                if change > 0 { "+" } else { "\u{2212}" },
                change.abs()
            )
        },
        #[allow(clippy::cast_precision_loss)]
        css_class: direction(change as f64),
        fill_pct: fill_pct(cur, prev),
    }
}

/// A magnitude delta stated as a percentage — the right form for pages and
/// seconds, where the absolute change carries no sense of scale. "New" when
/// the previous window recorded none of it at all.
fn percent_comparison(current: f64, previous: f64) -> Comparison {
    if previous <= 0.0 {
        return Comparison {
            label: if current > 0.0 { "new" } else { "flat" }.to_string(),
            css_class: if current > 0.0 { "up" } else { "flat" },
            fill_pct: fill_pct(current, previous),
        };
    }
    let change = (current - previous) / previous * 100.0;
    if change.abs() < 0.5 {
        return Comparison {
            label: "flat".to_string(),
            css_class: "flat",
            fill_pct: fill_pct(current, previous),
        };
    }
    // Display-only; the clamp keeps a divide-by-near-zero blowup from
    // saturating the cast.
    #[allow(clippy::cast_possible_truncation)]
    let pct = change.abs().round().clamp(0.0, f64::from(i32::MAX)) as i64;
    Comparison {
        label: format!("{}{pct}%", if change > 0.0 { "+" } else { "\u{2212}" }),
        css_class: direction(change),
        fill_pct: fill_pct(current, previous),
    }
}

/// A star delta stated in stars — "+0.1", "flat" — with the bar drawn against
/// the five-star ceiling rather than against the previous window. A rating is
/// already on a bounded scale, so its own share of that scale is the honest
/// shape; a ratio between two means would peg at 100% for any window that
/// rated as well as the last.
///
/// `None` when either window carries no rated books: a mean over nothing is
/// not zero, it is absent.
fn stars_comparison(current: Option<f64>, previous: Option<f64>) -> Option<Comparison> {
    let cur = current?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let fill_pct = (cur / 5.0 * 100.0).clamp(0.0, 100.0).round() as u32;
    let Some(prev) = previous else {
        return Some(Comparison {
            label: "new".to_string(),
            css_class: "up",
            fill_pct,
        });
    };
    let change = cur - prev;
    if change.abs() < 0.05 {
        return Some(Comparison {
            label: "flat".to_string(),
            css_class: "flat",
            fill_pct,
        });
    }
    Some(Comparison {
        label: format!(
            "{}{:.1}",
            if change > 0.0 { "+" } else { "\u{2212}" },
            change.abs()
        ),
        css_class: direction(change),
        fill_pct,
    })
}

/// Value + unit for a duration tile: minutes under an hour, one-decimal
/// hours under ten, whole hours beyond ("42" m · "3.5" h · "142" h).
fn duration_value(secs: i64) -> (String, &'static str) {
    if secs < 3600 {
        return ((secs / 60).to_string(), "min");
    }
    // Display-only: listening totals sit far below f64's 2^52 exact-integer
    // range.
    #[allow(clippy::cast_precision_loss)]
    let hours = secs as f64 / 3600.0;
    if hours < 10.0 {
        (format!("{hours:.1}"), "hours")
    } else {
        (format!("{:.0}", hours.floor()), "hours")
    }
}

/// One-decimal star mean, or the em-dash empty state — never NaN or 0.0.
/// Rounds half away from zero (explicit `round`) so a quarter-step mean like
/// 4.25 shows as 4.3, not the 4.2 that `{:.1}`'s round-half-to-even yields.
fn avg_stars_value(avg: Option<f64>) -> String {
    match avg {
        Some(stars) => format!("{:.1}", (stars * 10.0).round() / 10.0),
        None => "\u{2014}".to_string(),
    }
}

/// Thousand-grouped page count ("9,214"), or an empty state when
/// [`omnibus_shared::StatsSummary::pages_read`] is `None`.
///
/// Two different empty states, because they are two different facts. A window
/// whose only activity was listening turned exactly zero pages — audio has no
/// page analogue, so `0` is the answer, not a shrug. Anything else the ladder
/// could not measure is a genuine em-dash: something was read and the server
/// cannot say how much.
fn pages_value(pages: Option<i64>, audio_only: bool) -> String {
    match pages {
        Some(n) => group_thousands(n),
        None if audio_only => "0".to_string(),
        None => "\u{2014}".to_string(),
    }
}

/// One tile's content, assembled off the summary so the component stays a
/// renderer.
struct Tile {
    value: String,
    unit: &'static str,
    label: &'static str,
    testid: &'static str,
    metric: Metric,
    accent: bool,
    comparison: Option<Comparison>,
}

/// Every tile in reading order, with each metric's comparison already chosen.
/// The comparisons are dropped wholesale on Lifetime, which has no previous
/// window to measure against.
fn build_tiles(summary: &StatsSummary) -> Vec<Tile> {
    let compare = summary.range != StatsRange::AllTime;
    let previous: &PeriodComparison = &summary.previous;
    #[allow(clippy::cast_precision_loss)]
    let pages_now = summary.pages_read.unwrap_or(0) as f64;
    let (listen_value, listen_unit) = duration_value(summary.listening_seconds);
    #[allow(clippy::cast_precision_loss)]
    let listen_pair = (
        summary.listening_seconds as f64,
        previous.listening_seconds as f64,
    );
    vec![
        Tile {
            value: summary.books_finished.to_string(),
            unit: "books",
            label: "Finished",
            testid: "stats-tile-finished",
            metric: Metric::Finished,
            accent: true,
            comparison: compare
                .then(|| count_comparison(summary.books_finished, previous.books_finished)),
        },
        Tile {
            value: pages_value(summary.pages_read, summary.pages_detail.audio_only()),
            unit: "pages",
            label: "Pages read",
            testid: "stats-tile-pages",
            metric: Metric::Pages,
            accent: false,
            #[allow(clippy::cast_precision_loss)]
            comparison: compare.then(|| percent_comparison(pages_now, previous.pages_read as f64)),
        },
        Tile {
            value: listen_value,
            unit: listen_unit,
            label: "Listening",
            testid: "stats-tile-listening",
            metric: Metric::Listening,
            accent: false,
            comparison: compare.then(|| percent_comparison(listen_pair.0, listen_pair.1)),
        },
        Tile {
            value: avg_stars_value(summary.avg_stars),
            // No unit beside the em-dash: "— ★ avg" reads as a measured value
            // of nothing, where the bare dash reads as the absence it is.
            unit: if summary.avg_stars.is_some() {
                "\u{2605} avg"
            } else {
                ""
            },
            label: "How you rated",
            testid: "stats-tile-avg-rating",
            metric: Metric::AvgRating,
            accent: false,
            comparison: compare
                .then(|| stars_comparison(summary.avg_stars, previous.avg_stars))
                .flatten(),
        },
    ]
}

/// The four-tile headline row. Each tile is a button that opens its metric's
/// drill-in — the tile face carries the comparison, the drill-in carries the
/// trend and the caveats the face has no room for.
#[component]
pub(super) fn HeadlineTiles(summary: StatsSummary, expanded: Signal<Option<Metric>>) -> Element {
    let tiles = build_tiles(&summary);
    rsx! {
        div { class: "st-tiles",
            for tile in tiles {
                button {
                    key: "{tile.testid}",
                    class: if tile.accent { "st-tile accent" } else { "st-tile" },
                    "data-testid": tile.testid,
                    r#type: "button",
                    "aria-label": "Expand {tile.label} details",
                    onclick: move |_| expanded.set(Some(tile.metric)),
                    div { class: "st-tile-top",
                        // Keyed on the rendered value so a period switch
                        // remounts just this node when the new summary lands,
                        // replaying the content swap while the tile stays put.
                        span { key: "{tile.value}", class: "st-tile-value", {tile.value.clone()} }
                        // Omitted entirely rather than rendered empty: the
                        // row is a flex with an 8px gap, so an empty span
                        // still pushes the delta across.
                        if !tile.unit.is_empty() {
                            span { class: "st-tile-unit", {tile.unit} }
                        }
                        if let Some(c) = tile.comparison.as_ref() {
                            span {
                                class: "st-tile-delta {c.css_class}",
                                "data-testid": "{tile.testid}-delta",
                                {c.label.clone()}
                            }
                        }
                    }
                    div { class: "st-tile-label", {tile.label} }
                    if let Some(c) = tile.comparison.as_ref() {
                        div { class: "st-tile-track",
                            div { class: "st-tile-fill", style: "width: {c.fill_pct}%" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
