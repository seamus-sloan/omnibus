//! The generic chart renderer: one hand-rolled SVG that draws whatever
//! [`ChartResult`] it is handed.
//!
//! Deliberately not a chart library — every other chart in `pages/stats/` is
//! hand-rolled too, and a dependency here would style itself rather than
//! taking the page's tokens. Everything is derived from the prop, so SSR and
//! the first WASM paint produce identical markup (rule 07).

use dioxus::prelude::*;
use omnibus_shared::{ChartBucket, ChartMark, ChartResult, ChartSeries};

/// The logical drawing surface. The SVG scales to its container via CSS, so
/// these are the only coordinates any of the geometry below deals in.
const VIEW_W: f64 = 760.0;
const VIEW_H: f64 = 340.0;
const PAD_T: f64 = 14.0;
const PAD_B: f64 = 46.0;
const PAD_L: f64 = 56.0;
/// Right padding when only the left axis is drawn — just enough that the last
/// bar isn't flush against the edge.
const PAD_R_SINGLE: f64 = 18.0;
/// Right padding when a second axis needs room for its labels.
const PAD_R_DUAL: f64 = 56.0;
/// Horizontal share of a bucket's band that bars occupy, leaving the rest as
/// the gutter between buckets.
const BAR_SHARE: f64 = 0.68;
/// The same share once a band holds more than [`CROWDED_BARS`] series: the
/// gutter gives up room so six genre bars don't come out as hairlines.
const WIDE_BAR_SHARE: f64 = 0.88;
/// Bar count past which a band claims the wider share.
const CROWDED_BARS: usize = 2;
/// Surface gap between two bars sharing a band, so adjacent fills read as two
/// marks rather than one wide one.
const BAR_GAP: f64 = 2.0;
/// Below this slot width the full gap would eat most of the bar, so it halves.
const NARROW_SLOT: f64 = 7.0;
/// Gridline count used when a result names none — only reachable for a
/// hand-built result, since the builder always sends its own.
const FALLBACK_DIVISIONS: usize = 4;
/// The most x labels drawn; beyond this they are thinned to every Nth.
const MAX_X_LABELS: usize = 12;
/// How many distinct series colours the stylesheet defines (`--cb-s0`…).
const SERIES_COLOURS: usize = 6;

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// A bucket key rendered for the x-axis.
///
/// Kept total: a malformed key falls back to itself rather than panicking, so
/// a future bucket kind can't take the page down before its arm is written.
fn bucket_label(key: &str, bucket: ChartBucket) -> String {
    let part = |i: usize| key.split('-').nth(i).and_then(|p| p.parse::<usize>().ok());
    match bucket {
        ChartBucket::Year => key.to_string(),
        ChartBucket::Month => match (part(0), part(1)) {
            (Some(y), Some(m)) if (1..=12).contains(&m) => {
                // The year rides along on January so a multi-year axis stays
                // readable without repeating it on all twelve labels.
                if m == 1 {
                    format!("{} {}", MONTHS[m - 1], y % 100)
                } else {
                    MONTHS[m - 1].to_string()
                }
            }
            _ => key.to_string(),
        },
        ChartBucket::Day | ChartBucket::Week => match (part(1), part(2)) {
            (Some(m), Some(d)) if (1..=12).contains(&m) => format!("{d} {}", MONTHS[m - 1]),
            _ => key.to_string(),
        },
    }
}

/// An axis tick, trimmed to the precision the magnitude deserves.
fn tick_label(value: f64) -> String {
    if value >= 100.0 || value.fract().abs() < f64::EPSILON {
        format!("{}", value.round() as i64)
    } else if value >= 10.0 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

/// A plotted value, for the accessible summary table.
fn value_label(value: f64) -> String {
    if value.fract().abs() < 0.05 || value >= 100.0 {
        format!("{}", value.round() as i64)
    } else {
        format!("{value:.1}")
    }
}

/// Per-series geometry resolved once, so the markup below stays declarative.
struct Plot {
    pad_r: f64,
    plot_w: f64,
    plot_h: f64,
    bands: usize,
}

impl Plot {
    fn new(result: &ChartResult) -> Self {
        let pad_r = if result.axes.len() > 1 {
            PAD_R_DUAL
        } else {
            PAD_R_SINGLE
        };
        Self {
            pad_r,
            plot_w: VIEW_W - PAD_L - pad_r,
            plot_h: VIEW_H - PAD_T - PAD_B,
            bands: result.buckets.len().max(1),
        }
    }

    fn band_w(&self) -> f64 {
        self.plot_w / self.bands as f64
    }

    /// Centre of bucket `i`'s band.
    fn band_centre(&self, i: usize) -> f64 {
        PAD_L + self.band_w() * (i as f64 + 0.5)
    }

    /// Vertical position of `value` on `max`, clamped so a value overshooting
    /// its axis (impossible via `nice_ceiling`, but cheap to guard) still
    /// lands inside the frame.
    fn y(&self, value: f64, max: f64) -> f64 {
        let ratio = if max > 0.0 {
            (value / max).clamp(0.0, 1.0)
        } else {
            0.0
        };
        PAD_T + self.plot_h * (1.0 - ratio)
    }

    fn baseline(&self) -> f64 {
        PAD_T + self.plot_h
    }
}

/// Consecutive runs of present values, as `points` attributes.
///
/// A gap breaks the line rather than bridging it: an average with no data in
/// a bucket is unknown, and drawing straight through would assert a value
/// that was never measured.
fn line_runs(plot: &Plot, series: &ChartSeries, max: f64) -> Vec<String> {
    let mut runs: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for (i, value) in series.values.iter().enumerate() {
        match value {
            Some(v) => current.push(format!("{:.1},{:.1}", plot.band_centre(i), plot.y(*v, max))),
            None => {
                // A single point can't be a polyline; its dot carries it.
                if current.len() > 1 {
                    runs.push(current.join(" "));
                }
                current.clear();
            }
        }
    }
    if current.len() > 1 {
        runs.push(current.join(" "));
    }
    runs
}

/// The axis a series is scaled against, defaulting to the left when a result
/// somehow names an axis it did not send.
fn axis_max(result: &ChartResult, series: &ChartSeries) -> f64 {
    result
        .axes
        .get(series.axis as usize)
        .or_else(|| result.axes.first())
        .map(|a| a.max)
        .unwrap_or(1.0)
}

/// Render one chart.
#[component]
pub fn ChartPlot(result: ReadSignal<ChartResult>) -> Element {
    let result = result.read();
    let plot = Plot::new(&result);
    let band_w = plot.band_w();

    let bar_indices: Vec<usize> = result
        .series
        .iter()
        .enumerate()
        .filter(|(_, s)| s.mark == ChartMark::Bar)
        .map(|(i, _)| i)
        .collect();
    let bar_zone = band_w
        * if bar_indices.len() > CROWDED_BARS {
            WIDE_BAR_SHARE
        } else {
            BAR_SHARE
        };
    let bar_w = if bar_indices.is_empty() {
        0.0
    } else {
        bar_zone / bar_indices.len() as f64
    };
    let bar_gap = if bar_w < NARROW_SLOT {
        BAR_GAP / 2.0
    } else {
        BAR_GAP
    };

    // A result the builder produced always names its own; only a hand-built
    // one can arrive without.
    let divisions = match usize::from(result.divisions) {
        0 => FALLBACK_DIVISIONS,
        n => n,
    };
    let label_step = result.buckets.len().div_ceil(MAX_X_LABELS).max(1);
    let left_max = result.axes.first().map(|a| a.max).unwrap_or(1.0);
    let right_max = result.axes.get(1).map(|a| a.max);

    // One sentence naming what is plotted, for a reader who can't see it.
    let summary = result
        .series
        .iter()
        .map(ChartSeries::label)
        .collect::<Vec<_>>()
        .join(", ");

    rsx! {
        div { class: "cb-plot",
            svg {
                class: "cb-svg",
                view_box: "0 0 {VIEW_W} {VIEW_H}",
                role: "img",
                "aria-label": "Chart of {summary} over {result.buckets.len()} periods",
                preserve_aspect_ratio: "none",

                // ── Gridlines and the left axis ──────────────────────────
                for step in 0..=divisions {
                    {
                        let frac = step as f64 / divisions as f64;
                        let y = PAD_T + plot.plot_h * (1.0 - frac);
                        rsx! {
                            line {
                                class: "cb-grid",
                                x1: "{PAD_L}", x2: "{VIEW_W - plot.pad_r}",
                                y1: "{y:.1}", y2: "{y:.1}",
                            }
                            text {
                                class: "cb-tick cb-tick-left",
                                x: "{PAD_L - 10.0}", y: "{y + 4.0:.1}",
                                "text-anchor": "end",
                                "{tick_label(left_max * frac)}"
                            }
                            if let Some(rmax) = right_max {
                                text {
                                    class: "cb-tick cb-tick-right",
                                    x: "{VIEW_W - plot.pad_r + 10.0}", y: "{y + 4.0:.1}",
                                    "text-anchor": "start",
                                    "{tick_label(rmax * frac)}"
                                }
                            }
                        }
                    }
                }

                // ── Bars, one slot per bar series inside each band ───────
                for (slot, idx) in bar_indices.iter().enumerate() {
                    {
                        let series = &result.series[*idx];
                        let max = axis_max(&result, series);
                        let colour = *idx % SERIES_COLOURS;
                        rsx! {
                            g { class: "cb-bars cb-s{colour}",
                                for (i, value) in series.values.iter().enumerate() {
                                    if let Some(v) = value {
                                        {
                                            let top = plot.y(*v, max);
                                            let h = (plot.baseline() - top).max(0.0);
                                            let x = plot.band_centre(i) - bar_zone / 2.0
                                                + bar_w * slot as f64
                                                + bar_gap / 2.0;
                                            rsx! {
                                                rect {
                                                    class: "cb-bar",
                                                    x: "{x:.1}", y: "{top:.1}",
                                                    width: "{(bar_w - bar_gap).max(1.0):.1}",
                                                    height: "{h:.1}",
                                                    rx: "3",
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Lines, drawn over the bars ───────────────────────────
                for (idx, series) in result.series.iter().enumerate() {
                    if series.mark == ChartMark::Line {
                        {
                            let max = axis_max(&result, series);
                            let colour = idx % SERIES_COLOURS;
                            rsx! {
                                g { class: "cb-line cb-s{colour}",
                                    for points in line_runs(&plot, series, max) {
                                        polyline { class: "cb-stroke", points: "{points}" }
                                    }
                                    for (i, value) in series.values.iter().enumerate() {
                                        if let Some(v) = value {
                                            circle {
                                                class: "cb-dot",
                                                cx: "{plot.band_centre(i):.1}",
                                                cy: "{plot.y(*v, max):.1}",
                                                r: "4",
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // ── Baseline and x labels ────────────────────────────────
                line {
                    class: "cb-axis",
                    x1: "{PAD_L}", x2: "{VIEW_W - plot.pad_r}",
                    y1: "{plot.baseline():.1}", y2: "{plot.baseline():.1}",
                }
                for (i, key) in result.buckets.iter().enumerate() {
                    if i % label_step == 0 {
                        text {
                            class: "cb-xlabel",
                            x: "{plot.band_centre(i):.1}",
                            y: "{plot.baseline() + 20.0:.1}",
                            "text-anchor": "middle",
                            "{bucket_label(key, result.bucket)}"
                        }
                    }
                }
            }
        }
    }
}

/// The plotted numbers as a real, openable table.
///
/// Not a visually hidden one: three of the light-mode series colours sit below
/// 3:1 against the page, and the relief for that is a table a reader can
/// actually open — not one only a screen reader can reach.
#[component]
pub fn ChartTable(result: ReadSignal<ChartResult>) -> Element {
    let result = result.read();
    rsx! {
        details { class: "cb-table-wrap", "data-testid": "chart-table",
            summary { class: "cb-table-toggle", "Show the numbers" }
            table { class: "cb-table",
                caption { "Chart data" }
                thead {
                    tr {
                        th { scope: "col", "Period" }
                        for series in result.series.iter() {
                            th { scope: "col", "{series.label()}" }
                        }
                    }
                }
                tbody {
                    for (i, key) in result.buckets.iter().enumerate() {
                        tr {
                            th { scope: "row", "{bucket_label(key, result.bucket)}" }
                            for series in result.series.iter() {
                                td {
                                    match series.values.get(i).copied().flatten() {
                                        Some(v) => value_label(v),
                                        None => "no data".to_string(),
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The legend beneath the chart: one swatch per series, marked with the axis
/// it is scaled against when there are two.
#[component]
pub fn ChartLegend(result: ReadSignal<ChartResult>) -> Element {
    let result = result.read();
    let dual = result.axes.len() > 1;
    rsx! {
        ul { class: "cb-legend", "data-testid": "chart-legend",
            for (idx, series) in result.series.iter().enumerate() {
                li { class: "cb-legend-row",
                    span {
                        class: "cb-legend-swatch cb-s{idx % SERIES_COLOURS}",
                        "data-mark": match series.mark {
                            ChartMark::Bar => "bar",
                            ChartMark::Line => "line",
                        },
                    }
                    span { class: "cb-legend-name", "{series.label()}" }
                    if dual {
                        span { class: "cb-legend-axis",
                            if series.axis == 0 { "left" } else { "right" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
