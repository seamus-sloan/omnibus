//! Metric-tile drill-in: a bottom sheet on mobile, a centered modal on
//! desktop, switched by a CSS media query so the rsx stays identical on
//! every target (rule 07). Shows a vs-previous-period delta, the
//! metric's trend chart, and — per metric — the half-star distribution
//! (Avg rating) or the books completed in the window (Finished), all
//! from the already-fetched `StatsSummary` (no new RPC).

use dioxus::prelude::*;
use omnibus_shared::{
    Contributor, EbookMetadata, FinishedBook, RatingBucket, StatsRange, StatsSummary,
};

use crate::components::{ConfirmModal, CoverTile, CoverTileKind};
use crate::use_server_url;

/// Which headline tile a drill-in sheet/modal is showing detail for.
#[derive(Clone, Copy, PartialEq)]
pub(super) enum Metric {
    Finished,
    AvgRating,
    Listening,
    Pages,
}

impl Metric {
    /// Title shown in the drill-in header.
    fn title(self) -> &'static str {
        match self {
            Metric::Finished => "Finished",
            Metric::AvgRating => "Avg rating",
            Metric::Listening => "Listening",
            Metric::Pages => "Est. pages",
        }
    }
}

/// A formatted vs-previous-period delta: direction glyph + magnitude label.
struct Delta {
    glyph: &'static str,
    css_class: &'static str,
    label: String,
}

/// "vs last week/month/year" — empty for Lifetime, which has no previous
/// window to compare against.
fn vs_label(range: StatsRange) -> &'static str {
    match range {
        StatsRange::Week => "vs last week",
        StatsRange::Month => "vs last month",
        StatsRange::Year => "vs last year",
        StatsRange::AllTime => "",
    }
}

/// Percent change from `previous` to `current`, `None` when there's nothing
/// to compare (no previous-period baseline and nothing new either).
fn percent_delta(current: f64, previous: f64) -> Option<Delta> {
    if previous <= 0.0 {
        return (current > 0.0).then(|| Delta {
            glyph: "\u{25B2}",
            css_class: "up",
            label: "New".to_string(),
        });
    }
    let change = (current - previous) / previous * 100.0;
    if change.abs() < 0.5 {
        return Some(Delta {
            glyph: "\u{25CF}",
            css_class: "flat",
            label: "No change".to_string(),
        });
    }
    // Display-only percentage; the clamp keeps a divide-by-near-zero blowup
    // from saturating the cast.
    #[allow(clippy::cast_possible_truncation)]
    let pct = change.abs().round().clamp(0.0, f64::from(i32::MAX)) as i64;
    Some(if change > 0.0 {
        Delta {
            glyph: "\u{25B2}",
            css_class: "up",
            label: format!("{pct}%"),
        }
    } else {
        Delta {
            glyph: "\u{25BC}",
            css_class: "down",
            label: format!("{pct}%"),
        }
    })
}

/// Absolute star-rating change, `None` when there's no baseline to compare
/// (either window carries no rated books).
fn stars_delta(current: Option<f64>, previous: Option<f64>) -> Option<Delta> {
    let (cur, prev) = (current?, previous?);
    let change = cur - prev;
    if change.abs() < 0.05 {
        return Some(Delta {
            glyph: "\u{25CF}",
            css_class: "flat",
            label: "No change".to_string(),
        });
    }
    let label = format!("{:.1}\u{2605}", change.abs());
    Some(if change > 0.0 {
        Delta {
            glyph: "\u{25B2}",
            css_class: "up",
            label,
        }
    } else {
        Delta {
            glyph: "\u{25BC}",
            css_class: "down",
            label,
        }
    })
}

/// One rendered trend bar: a short axis label, the hover title, and a height
/// 0..=100 relative to the series' tallest point. An all-zero series stays all
/// zero.
struct TrendBar {
    label: String,
    /// Hover text. Defaults to the axis label; a caller with something more
    /// useful to say (the histogram's book count) overwrites it.
    title: String,
    height_pct: u32,
}

/// Normalize any of the summary's label/value series into bar heights.
fn build_trend_bars(points: &[(String, f64)]) -> Vec<TrendBar> {
    let max = points.iter().map(|(_, v)| *v).fold(0.0_f64, f64::max);
    points
        .iter()
        .map(|(label, value)| TrendBar {
            label: label.clone(),
            title: label.clone(),
            height_pct: if max <= 0.0 {
                0
            } else {
                // `max` is the series maximum, so the ratio is 0..=1 and the
                // scaled value 0..=100.
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                let pct = ((value.max(0.0) / max) * 100.0).round().clamp(0.0, 100.0) as u32;
                pct
            },
        })
        .collect()
}

/// A rating bucket's axis label, in **stars** — "0.5", "1", "1.5" … "5".
/// Ratings are stored as half-stars 1..=10; labelling the raw value would
/// present the chart as a ten-point scale.
fn star_label(bucket: &RatingBucket) -> String {
    let stars = bucket.stars();
    if bucket.half_stars % 2 == 0 {
        format!("{stars:.0}")
    } else {
        format!("{stars:.1}")
    }
}

/// The window's ratings as bars, one per half-star bucket, with the book count
/// on hover. Empty when the window carries no ratings at all — the caller
/// renders its empty state rather than ten flat bars.
// Display-only heights: bucket counts sit far below f64's 2^52 exact-integer
// range.
#[allow(clippy::cast_precision_loss)]
fn build_histogram_bars(buckets: &[RatingBucket]) -> Vec<TrendBar> {
    let points: Vec<(String, f64)> = buckets
        .iter()
        .map(|b| (star_label(b), b.books as f64))
        .collect();
    let mut bars = build_trend_bars(&points);
    for (bar, bucket) in bars.iter_mut().zip(buckets) {
        let plural = if bucket.books == 1 { "" } else { "s" };
        bar.title = format!(
            "{} \u{2605} \u{00B7} {} book{plural}",
            bar.label, bucket.books
        );
    }
    bars
}

/// The metric's trend series as `(short label, value)` pairs, drawn from the
/// fields already on `summary` — no metric needs a fresh fetch to drill in.
// Display-only trend values: counts and seconds sit far below f64's 2^52
// exact-integer range.
#[allow(clippy::cast_precision_loss)]
fn trend_points(metric: Metric, summary: &StatsSummary) -> Vec<(String, f64)> {
    match metric {
        Metric::Finished => summary
            .books_per_month
            .iter()
            .map(|m| (short_month(&m.month), m.books as f64))
            .collect(),
        Metric::AvgRating => summary
            .rating_monthly
            .iter()
            .map(|p| (short_month(&p.label), p.value))
            .collect(),
        Metric::Listening => summary
            .listening_daily
            .iter()
            .map(|d| (short_day(&d.day), d.seconds as f64 / 60.0))
            .collect(),
        Metric::Pages => Vec::new(),
    }
}

/// First letter of a `YYYY-MM` month, `?` when malformed.
fn short_month(month: &str) -> String {
    month
        .rsplit('-')
        .next()
        .and_then(|m| m.parse::<usize>().ok())
        .filter(|m| (1..=12).contains(m))
        .map(|m| {
            const INITIALS: [char; 12] =
                ['J', 'F', 'M', 'A', 'M', 'J', 'J', 'A', 'S', 'O', 'N', 'D'];
            INITIALS[m - 1].to_string()
        })
        .unwrap_or_else(|| "?".to_string())
}

/// Day-of-month for a `YYYY-MM-DD` day, `?` when malformed.
fn short_day(day: &str) -> String {
    day.rsplit('-')
        .next()
        .filter(|_| day.contains('-'))
        .and_then(|d| d.parse::<usize>().ok())
        .filter(|d| (1..=31).contains(d))
        .map(|d| format!("{d:02}"))
        .unwrap_or_else(|| "?".to_string())
}

/// The metric's current/previous values as `(current, previous)` for the
/// percent-based deltas (Finished, Listening). Avg rating uses
/// `stars_delta` directly since it's `Option<f64>` on both sides.
// Display-only deltas: counts and seconds sit far below f64's 2^52
// exact-integer range.
#[allow(clippy::cast_precision_loss)]
fn delta_for(metric: Metric, summary: &StatsSummary, range: StatsRange) -> Option<Delta> {
    if range == StatsRange::AllTime {
        return None;
    }
    match metric {
        Metric::Finished => percent_delta(
            summary.books_finished as f64,
            summary.previous.books_finished as f64,
        ),
        Metric::AvgRating => stars_delta(summary.avg_stars, summary.previous.avg_stars),
        Metric::Listening => percent_delta(
            summary.listening_seconds as f64,
            summary.previous.listening_seconds as f64,
        ),
        Metric::Pages => None,
    }
}

/// Build a minimal `EbookMetadata` from a `FinishedBook` row so the drill-in
/// list can hand it to the shared `CoverTile` — the DTO only carries the
/// handful of fields a cover + title row needs.
fn finished_book_as_ebook(book: &FinishedBook) -> EbookMetadata {
    EbookMetadata {
        title: Some(book.title.clone()),
        filename: book.title.clone(),
        creators: book
            .author
            .clone()
            .map(|name| {
                vec![Contributor {
                    name,
                    role: None,
                    file_as: None,
                    id: None,
                }]
            })
            .unwrap_or_default(),
        unique_identifier: Some(book.book_uuid.clone()),
        cover_url: book.cover_url.clone(),
        ..Default::default()
    }
}

/// The drill-in sheet/modal: header + close, delta chip, trend chart, and
/// (Finished only) the finished-books list. `expanded` closes on backdrop
/// click or the close button. Built on the shared `ConfirmModal` shell (see
/// `components::confirm_modal`) — `busy: false` since no mutation is ever
/// in flight here, the grabber + title/close head go in the `head` slot,
/// and `backdrop_class`/`dialog_class` keep this sheet's own bottom-sheet
/// (mobile) / centered-modal (desktop) chrome rather than the default
/// author-photo backdrop.
#[component]
pub(super) fn DrillIn(
    metric: Metric,
    summary: StatsSummary,
    range: StatsRange,
    expanded: Signal<Option<Metric>>,
) -> Element {
    let server_url = use_server_url();
    let delta = delta_for(metric, &summary, range);
    let vs = vs_label(range);
    let bars = build_trend_bars(&trend_points(metric, &summary));

    rsx! {
        ConfirmModal {
            testid: "stats-drill-in".to_string(),
            aria_label: "{metric.title()} detail",
            backdrop_class: "st-drill-scrim".to_string(),
            dialog_class: "st-drill-sheet".to_string(),
            busy: false,
            on_dismiss: move |_| expanded.set(None),
            head: rsx! {
                div { class: "st-drill-grabber" }
                div { class: "st-drill-head",
                    h4 { "{metric.title()}" }
                    button {
                        class: "st-drill-close",
                        "data-testid": "stats-drill-close",
                        r#type: "button",
                        "aria-label": "Close",
                        onclick: move |_| expanded.set(None),
                        "\u{2715}"
                    }
                }
            },
            div { class: "st-drill-body",
                {render_delta(delta, vs)}
                {render_trend(metric, &bars)}
                if metric == Metric::AvgRating {
                    {render_histogram(&summary.rating_histogram)}
                }
                if metric == Metric::Finished {
                    {render_finished_list(&summary.finished_books, summary.books_finished, &server_url)}
                }
            }
        }
    }
}

/// The delta chip, or a friendly "not enough data" line when there's no
/// baseline (fresh metric, or the Lifetime range with no previous window).
fn render_delta(delta: Option<Delta>, vs: &str) -> Element {
    match delta {
        Some(d) => rsx! {
            div {
                class: "st-drill-delta {d.css_class}",
                "data-testid": "stats-drill-delta",
                span { aria_hidden: "true", "{d.glyph}" }
                " {d.label} "
                if !vs.is_empty() {
                    span { class: "mono st-drill-delta-vs", "{vs}" }
                }
            }
        },
        None => rsx! {
            p { class: "st-drill-delta-empty", "data-testid": "stats-drill-delta",
                "Not enough data yet to compare."
            }
        },
    }
}

/// The shared pure-CSS bar strip: one normalized column per point, with its
/// axis label beneath. Both the metric trend and the rating histogram render
/// through this — the histogram is the same widget with a different x-axis, so
/// a second bar renderer would only be a second thing to keep in sync.
fn render_bars(bars: &[TrendBar], testid: &str, aria_label: &str) -> Element {
    rsx! {
        div {
            class: "st-drill-trend",
            "data-testid": "{testid}",
            role: "img",
            aria_label: "{aria_label}",
            for (i, bar) in bars.iter().enumerate() {
                div { key: "{i}-{bar.label}", class: "st-drill-trend-col", title: "{bar.title}",
                    div { class: "st-drill-trend-track",
                        div { class: "st-drill-trend-bar", style: "height: {bar.height_pct}%;" }
                    }
                    div {
                        class: "st-drill-trend-label mono",
                        "data-testid": "stats-drill-bar-label",
                        "{bar.label}"
                    }
                }
            }
        }
    }
}

/// The metric's trend chart — pure-CSS bar columns — or a placeholder note
/// for the Pages tile. The headline tile carries a real estimate,
/// but a day/month-bucketed trend series over it isn't computed yet.
fn render_trend(metric: Metric, bars: &[TrendBar]) -> Element {
    if metric == Metric::Pages {
        return rsx! {
            p { class: "st-drill-delta-empty", "Page trend tracking isn\u{2019}t available yet." }
        };
    }
    if bars.is_empty() {
        return rsx! { div {} };
    }
    render_bars(
        bars,
        "stats-drill-trend",
        &format!("{} trend", metric.title()),
    )
}

/// The Avg rating drill-in's distribution: how many books landed in each
/// half-star bucket. The mean above it can't tell a reader who rates
/// everything 4 from one who splits evenly between 2 and 5 — this can.
fn render_histogram(buckets: &[RatingBucket]) -> Element {
    if buckets.iter().all(|b| b.books == 0) {
        return rsx! {
            p { class: "st-drill-delta-empty", "data-testid": "stats-drill-histogram-empty",
                "No ratings in this window yet."
            }
        };
    }
    rsx! {
        div { class: "label st-drill-section-label", "Books at each rating" }
        {render_bars(&build_histogram_bars(buckets), "stats-drill-histogram", "Star rating distribution")}
    }
}

/// The "what you finished" rail: cover (via `CoverTile`) + title + rating
/// per book completed in the window. The server caps the list at its newest
/// N completions while `total` is the uncapped count, so a truncated rail
/// labels itself instead of silently posing as exhaustive.
fn render_finished_list(books: &[FinishedBook], total: i64, server_url: &str) -> Element {
    if books.is_empty() {
        return rsx! {
            p { class: "st-drill-delta-empty", "No books finished in this window." }
        };
    }
    let shown = books.len() as i64;
    rsx! {
        ul { class: "st-drill-finished-list", "data-testid": "stats-drill-finished-list",
            for book in books {
                {render_finished_row(book, server_url)}
            }
        }
        if total > shown {
            p { class: "st-drill-delta-empty", "Showing the latest {shown} of {total} finished books." }
        }
    }
}

fn render_finished_row(book: &FinishedBook, server_url: &str) -> Element {
    let ebook = finished_book_as_ebook(book);
    let rating_label = match book.rating {
        Some(r) => format!("{r:.1} \u{2605}"),
        None => "\u{2014}".to_string(),
    };
    rsx! {
        li { key: "{book.book_uuid}", class: "st-drill-finished-row",
            div { class: "st-drill-finished-cover",
                CoverTile {
                    book: ebook,
                    server_url: server_url.to_string(),
                    sizes: "40px".to_string(),
                    kind: CoverTileKind::ReadOnly,
                }
            }
            div { class: "st-drill-finished-body",
                div { class: "st-drill-finished-title", "{book.title}" }
                if let Some(author) = &book.author {
                    div { class: "mono st-drill-finished-author", "{author}" }
                }
            }
            div { class: "st-drill-finished-rating mono", "{rating_label}" }
        }
    }
}

#[cfg(test)]
mod tests;
