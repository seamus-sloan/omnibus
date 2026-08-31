//! The generic chart renderer: one hand-rolled SVG that draws whatever
//! [`ChartResult`] it is handed, plus its legend, hover readout and data table.
//!
//! Deliberately not a chart library — every other chart in `pages/stats/` is
//! hand-rolled too, and a dependency here would style itself rather than
//! taking the page's tokens. Every mark's geometry is derived from the prop,
//! and the hover state starts empty on every target, so SSR and the first WASM
//! paint produce identical markup (rule 07).

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
/// The same share once a band holds more than [`CROWDED_BARS`] slots: the
/// gutter gives up room so six grouped bars don't come out as hairlines.
const WIDE_BAR_SHARE: f64 = 0.88;
/// Slot count past which a band claims the wider share.
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
/// Entrance stagger per bucket, capped so a wide chart still settles quickly.
const STAGGER_MS: u32 = 18;
const STAGGER_CAP_MS: u32 = 320;
/// Share of the width past which the hover card flips to the other side of the
/// cursor, so it never hangs off the plot.
const TIP_FLIP_PCT: f64 = 55.0;

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

/// The full bucket name for the hover card, which has room for the year the
/// axis label drops.
fn bucket_title(key: &str, bucket: ChartBucket) -> String {
    let part = |i: usize| key.split('-').nth(i).and_then(|p| p.parse::<usize>().ok());
    match bucket {
        ChartBucket::Month => match (part(0), part(1)) {
            (Some(y), Some(m)) if (1..=12).contains(&m) => format!("{} {y}", MONTHS[m - 1]),
            _ => key.to_string(),
        },
        ChartBucket::Week => format!("Week of {}", bucket_label(key, bucket)),
        _ => bucket_label(key, bucket),
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

/// A plotted value, for the hover card and the data table.
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
    /// its axis (impossible via the builder's fitted axis, but cheap to guard)
    /// still lands inside the frame.
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

/// A point on a line, in view coordinates.
#[derive(Clone, Copy)]
struct Pt {
    x: f64,
    y: f64,
}

/// One unbroken run of a line series.
struct Run {
    /// `M`/`C` path data.
    path: String,
    /// The same run closed down to the baseline — a soft wash that gives a
    /// thin line presence without competing with the bars.
    area: String,
}

/// Monotone-cubic path through `pts`, the interpolation `d3.curveMonotoneX`
/// uses.
///
/// Chosen over a plain cardinal spline because it **cannot overshoot**: a
/// curve that bulged past its own data would draw an average higher than any
/// month actually recorded, which is a chart lying to look smooth. Two points
/// fall back to a straight segment, where no curve is defined.
fn monotone_path(pts: &[Pt]) -> String {
    if pts.len() < 2 {
        return String::new();
    }
    let mut d = format!("M{:.1},{:.1}", pts[0].x, pts[0].y);
    if pts.len() == 2 {
        d.push_str(&format!(" L{:.1},{:.1}", pts[1].x, pts[1].y));
        return d;
    }

    let n = pts.len();
    let secants: Vec<f64> = (0..n - 1)
        .map(|i| {
            let dx = pts[i + 1].x - pts[i].x;
            if dx.abs() < f64::EPSILON {
                0.0
            } else {
                (pts[i + 1].y - pts[i].y) / dx
            }
        })
        .collect();

    // Fritsch-Carlson tangents: zero at every local extremum, which is what
    // keeps the curve inside the data's own range.
    let mut tangents = vec![0.0; n];
    tangents[0] = secants[0];
    tangents[n - 1] = secants[n - 2];
    for i in 1..n - 1 {
        let (a, b) = (secants[i - 1], secants[i]);
        tangents[i] = if a * b <= 0.0 {
            0.0
        } else {
            // Clamped to three times the shallower secant, so a steep
            // neighbour can't drag the curve past the point it passes through.
            let limit = 3.0 * a.abs().min(b.abs());
            ((a + b) / 2.0).clamp(-limit, limit)
        };
    }

    for i in 0..n - 1 {
        let dx = pts[i + 1].x - pts[i].x;
        let c1x = pts[i].x + dx / 3.0;
        let c1y = pts[i].y + tangents[i] * dx / 3.0;
        let c2x = pts[i + 1].x - dx / 3.0;
        let c2y = pts[i + 1].y - tangents[i + 1] * dx / 3.0;
        d.push_str(&format!(
            " C{c1x:.1},{c1y:.1} {c2x:.1},{c2y:.1} {:.1},{:.1}",
            pts[i + 1].x,
            pts[i + 1].y
        ));
    }
    d
}

/// Consecutive runs of present values, as curved paths plus their areas.
///
/// A gap breaks the line rather than bridging it: an average with no data in
/// a bucket is unknown, and drawing through would assert a value that was
/// never measured.
fn line_runs(plot: &Plot, series: &ChartSeries, max: f64) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    let mut current: Vec<Pt> = Vec::new();
    let mut flush = |pts: &mut Vec<Pt>| {
        if pts.len() > 1 {
            let path = monotone_path(pts);
            let area = format!(
                "{path} L{:.1},{:.1} L{:.1},{:.1} Z",
                pts[pts.len() - 1].x,
                plot.baseline(),
                pts[0].x,
                plot.baseline()
            );
            runs.push(Run { path, area });
        }
        pts.clear();
    };
    for (i, value) in series.values.iter().enumerate() {
        match value {
            Some(v) => current.push(Pt {
                x: plot.band_centre(i),
                y: plot.y(*v, max),
            }),
            // A single point can't be a path; its dot carries it.
            None => flush(&mut current),
        }
    }
    flush(&mut current);
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

/// The running total beneath each stacked bar, per series per bucket.
///
/// Returned up front rather than accumulated inline so the markup stays a pure
/// function of position. An absent value contributes nothing to the stack —
/// the bar above it simply sits lower, rather than the column gaining a hole.
fn stack_offsets(result: &ChartResult, bar_indices: &[usize]) -> Vec<Vec<f64>> {
    let mut running = vec![0.0; result.buckets.len()];
    bar_indices
        .iter()
        .map(|idx| {
            let base = running.clone();
            for (b, r) in running.iter_mut().enumerate() {
                *r += result.series[*idx]
                    .values
                    .get(b)
                    .copied()
                    .flatten()
                    .unwrap_or(0.0);
            }
            base
        })
        .collect()
}

/// Render one chart.
#[component]
pub fn ChartPlot(result: ReadSignal<ChartResult>) -> Element {
    // Seeded empty on every target so SSR and the first WASM paint agree
    // (rule 07); a hover only ever arrives from a client pointer.
    let mut hovered: Signal<Option<usize>> = use_signal(|| None);
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
    // Stacked bars share one slot per bucket; grouped ones split the band.
    let slots = if result.stacked { 1 } else { bar_indices.len() };
    let bar_zone = band_w
        * if slots > CROWDED_BARS {
            WIDE_BAR_SHARE
        } else {
            BAR_SHARE
        };
    let bar_w = if slots == 0 {
        0.0
    } else {
        bar_zone / slots as f64
    };
    let bar_gap = if bar_w < NARROW_SLOT {
        BAR_GAP / 2.0
    } else {
        BAR_GAP
    };
    let offsets = if result.stacked {
        stack_offsets(&result, &bar_indices)
    } else {
        Vec::new()
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
    let active = hovered();

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
                onmouseleave: move |_| hovered.set(None),

                // ── Gridlines and the axes' ticks ────────────────────────
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

                // ── The hovered band, behind every mark ──────────────────
                if let Some(i) = active {
                    rect {
                        class: "cb-band",
                        x: "{plot.band_centre(i) - band_w / 2.0:.1}",
                        y: "{PAD_T}",
                        width: "{band_w:.1}",
                        height: "{plot.plot_h:.1}",
                    }
                }

                // ── Bars ────────────────────────────────────────────────
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
                                            // A stacked bar starts on the
                                            // running total beneath it; a
                                            // grouped one starts at zero and
                                            // takes its own lane in the band.
                                            let base = offsets
                                                .get(slot)
                                                .and_then(|o| o.get(i))
                                                .copied()
                                                .unwrap_or(0.0);
                                            let top = plot.y(base + *v, max);
                                            let h = (plot.y(base, max) - top).max(0.0);
                                            let lane =
                                                if result.stacked { 0.0 } else { slot as f64 };
                                            let x = plot.band_centre(i) - bar_zone / 2.0
                                                + bar_w * lane
                                                + bar_gap / 2.0;
                                            let delay =
                                                (i as u32 * STAGGER_MS).min(STAGGER_CAP_MS);
                                            let dim = active.is_some_and(|a| a != i);
                                            rsx! {
                                                rect {
                                                    class: if dim { "cb-bar is-dim" } else { "cb-bar" },
                                                    x: "{x:.1}", y: "{top:.1}",
                                                    width: "{(bar_w - bar_gap).max(1.0):.1}",
                                                    height: "{h:.1}",
                                                    rx: "3",
                                                    style: "--cb-delay: {delay}ms",
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
                                    for run in line_runs(&plot, series, max) {
                                        path { class: "cb-area", d: "{run.area}" }
                                        path { class: "cb-stroke", d: "{run.path}" }
                                    }
                                    for (i, value) in series.values.iter().enumerate() {
                                        if let Some(v) = value {
                                            circle {
                                                class: if active == Some(i) { "cb-dot is-on" } else { "cb-dot" },
                                                cx: "{plot.band_centre(i):.1}",
                                                cy: "{plot.y(*v, max):.1}",
                                                r: if active == Some(i) { "5.5" } else { "4" },
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
                    // A thinned-out label reappears while its bucket is
                    // hovered, so the pointer always has one to read.
                    if i % label_step == 0 || active == Some(i) {
                        text {
                            class: if active == Some(i) { "cb-xlabel is-on" } else { "cb-xlabel" },
                            x: "{plot.band_centre(i):.1}",
                            y: "{plot.baseline() + 20.0:.1}",
                            "text-anchor": "middle",
                            "{bucket_label(key, result.bucket)}"
                        }
                    }
                }

                // ── Hit targets, above everything ────────────────────────
                // One full-height rect per bucket rather than one per mark: a
                // reader aiming at a short bar shouldn't have to hit the bar,
                // and the card reads every series at once anyway.
                for i in 0..result.buckets.len() {
                    rect {
                        class: "cb-hit",
                        x: "{plot.band_centre(i) - band_w / 2.0:.1}",
                        y: "{PAD_T}",
                        width: "{band_w:.1}",
                        height: "{plot.plot_h:.1}",
                        onmouseenter: move |_| hovered.set(Some(i)),
                    }
                }
            }

            if let Some(i) = active {
                ChartHoverCard {
                    result: ChartResult::clone(&result),
                    index: i,
                }
            }
        }
    }
}

/// The hovered bucket's readout: every series at once, so a reader can compare
/// them without measuring heights against two different scales.
///
/// Positioned as a share of the plot's width rather than in view units,
/// because the SVG stretches to its container and this is HTML sitting over it.
#[component]
fn ChartHoverCard(result: ReadSignal<ChartResult>, index: usize) -> Element {
    let result = result.read();
    let Some(key) = result.buckets.get(index) else {
        return rsx! {};
    };
    let plot = Plot::new(&result);
    let pct = plot.band_centre(index) / VIEW_W * 100.0;
    let flip = pct > TIP_FLIP_PCT;

    rsx! {
        div {
            class: if flip { "cb-tip is-flipped" } else { "cb-tip" },
            "data-testid": "chart-tooltip",
            role: "status",
            style: "left: {pct:.2}%",
            p { class: "cb-tip-head", "{bucket_title(key, result.bucket)}" }
            ul { class: "cb-tip-rows",
                for (idx, series) in result.series.iter().enumerate() {
                    // In a stacked chart a zero slice draws no segment, so
                    // listing it describes something that isn't on screen —
                    // and with six genres that is most of the card. Elsewhere
                    // a zero is the measure's real value and stays.
                    if !(result.stacked
                        && series.values.get(index).copied().flatten() == Some(0.0))
                    {
                    li { class: "cb-tip-row",
                        span { class: "cb-tip-swatch cb-s{idx % SERIES_COLOURS}" }
                        span { class: "cb-tip-name", "{series.label()}" }
                        span { class: "cb-tip-value",
                            match series.values.get(index).copied().flatten() {
                                Some(v) => format!(
                                    "{} {}",
                                    value_label(v),
                                    series.measure.unit().label()
                                ),
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

#[cfg(all(test, feature = "server"))]
mod tests;
