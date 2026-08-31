//! The configurable chart builder page (`/stats/chart`).
//!
//! A standalone surface while `/stats` is being redesigned; the intent is for
//! it to become the "customize" escape hatch behind the curated cards, with
//! those cards re-expressed as presets.
//!
//! The picker only ever composes a [`ChartSpec`] out of closed enums — it
//! never names a column or an aggregate — and the server re-validates what it
//! sends. The controls below are a convenience on top of that contract, not
//! the contract itself.

use dioxus::prelude::*;
use omnibus_shared::{
    ChartBreakdown, ChartBucket, ChartMeasure, ChartResult, ChartSpec, StatsRange,
};

use crate::components::{PageError, PageLoading};
use crate::{data, use_page_title, use_server_url, Route};

mod plot;

use plot::{ChartLegend, ChartPlot, ChartTable};

/// Sentinel for the "no second measure" option. Not a `ChartMeasure` variant:
/// absence is a property of the *selection*, not a thing that can be plotted.
const NONE_VALUE: &str = "__none";

/// Fetch the current spec's series, dropping any answer a newer spec has
/// already superseded.
fn use_chart_fetch_effect(
    server_url: String,
    spec: Signal<ChartSpec>,
    result: Signal<Option<ChartResult>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
) {
    let mut epoch = use_signal(|| 0u64);
    use_effect(move || {
        let current = spec();
        let ticket = *epoch.peek() + 1;
        epoch.set(ticket);
        let url = server_url.clone();
        let mut result = result;
        let mut loading = loading;
        let mut error = error;
        loading.set(true);
        spawn(async move {
            let answer = data::fetch_chart_series(&url, current).await;
            // A newer selection landed while this was in flight; its answer
            // is the one the page is waiting for.
            if *epoch.peek() != ticket {
                return;
            }
            match answer {
                Ok(r) => {
                    result.set(Some(r));
                    error.set(None);
                }
                // The message is the server's own, which for a rejected spec
                // names what to change; anything else is already generalized.
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });
}

/// The chart-builder page.
#[component]
pub fn ChartBuilderPage() -> Element {
    use_page_title(|| Some("Chart builder".into()));
    let server_url = use_server_url();
    // Seeded to the same default on every target so SSR and the first WASM
    // paint agree (rule 07).
    let spec = use_signal(ChartSpec::default);
    let result: Signal<Option<ChartResult>> = use_signal(|| None);
    let loading = use_signal(|| true);
    let error: Signal<Option<String>> = use_signal(|| None);

    use_chart_fetch_effect(server_url.clone(), spec, result, loading, error);

    rsx! {
        div { class: "cb-page", "data-testid": "chart-builder",
            header { class: "cb-masthead",
                h1 { class: "cb-title", "Chart builder" }
                p { class: "cb-sub",
                    "Pick what to plot. Every measure is computed over its own "
                    "table and lined up on a shared time bucket, so two that "
                    "count different things can still share an axis."
                }
            }
            ChartControls { spec }
            ChartCanvas { result, loading, error }
        }
    }
}

/// The picker. Each control writes one field of the spec; the effect above
/// turns any write into a refetch.
#[component]
fn ChartControls(spec: Signal<ChartSpec>) -> Element {
    let current = spec.read().clone();
    let primary = current
        .measures
        .first()
        .copied()
        .unwrap_or_default_measure();
    let secondary = current.measures.get(1).copied();

    rsx! {
        section { class: "cb-controls", "data-testid": "chart-controls",
            div { class: "cb-field",
                label { r#for: "cb-measure-a", "Measure" }
                select {
                    id: "cb-measure-a",
                    class: "cb-select",
                    onchange: move |e| {
                        if let Some(m) = ChartMeasure::from_query(&e.value()) {
                            let mut next = spec.read().clone();
                            // Replacing the primary with the measure already
                            // in the second slot would plot it twice, so the
                            // comparison drops rather than duplicating.
                            if next.measures.get(1) == Some(&m) {
                                next.measures.truncate(1);
                            }
                            if next.measures.is_empty() {
                                next.measures.push(m);
                            } else {
                                next.measures[0] = m;
                            }
                            if !m.supports_breakdown() {
                                next.breakdown = ChartBreakdown::None;
                            }
                            spec.set(next);
                        }
                    },
                    for m in ChartMeasure::ALL {
                        option {
                            value: "{m.as_query()}",
                            selected: m == primary,
                            "{m.label()}"
                        }
                    }
                }
                span { class: "cb-hint", "measured {primary.grain().label()}" }
            }

            div { class: "cb-field",
                label { r#for: "cb-measure-b", "Compare with" }
                select {
                    id: "cb-measure-b",
                    class: "cb-select",
                    onchange: move |e| {
                        let raw = e.value();
                        let mut next = spec.read().clone();
                        next.measures.truncate(1);
                        if let Some(m) = ChartMeasure::from_query(&raw) {
                            next.measures.push(m);
                            // A second measure needs both axes, leaving no
                            // room for a split (rule: a breakdown is a
                            // single-measure chart).
                            next.breakdown = ChartBreakdown::None;
                        }
                        spec.set(next);
                    },
                    option {
                        value: "{NONE_VALUE}",
                        selected: secondary.is_none(),
                        "Nothing"
                    }
                    // The primary is excluded rather than offered and then
                    // rejected — the same measure twice is not a comparison.
                    for m in ChartMeasure::ALL.into_iter().filter(|m| *m != primary) {
                        option {
                            value: "{m.as_query()}",
                            selected: Some(m) == secondary,
                            "{m.label()}"
                        }
                    }
                }
                span { class: "cb-hint",
                    match secondary {
                        Some(m) => format!("measured {}", m.grain().label()),
                        None => "one measure, one axis".to_string(),
                    }
                }
            }

            div { class: "cb-field",
                label { r#for: "cb-bucket", "Group by" }
                select {
                    id: "cb-bucket",
                    class: "cb-select",
                    onchange: move |e| {
                        let raw = e.value();
                        if let Some(b) = ChartBucket::ALL.into_iter().find(|b| b.as_query() == raw) {
                            let mut next = spec.read().clone();
                            next.bucket = b;
                            spec.set(next);
                        }
                    },
                    for b in ChartBucket::ALL {
                        option {
                            value: "{b.as_query()}",
                            selected: b == current.bucket,
                            "{b.label()}"
                        }
                    }
                }
            }

            div { class: "cb-field",
                label { r#for: "cb-range", "Period" }
                select {
                    id: "cb-range",
                    class: "cb-select",
                    onchange: move |e| {
                        let raw = e.value();
                        if let Some(r) = StatsRange::ALL.into_iter().find(|r| r.as_query() == raw) {
                            let mut next = spec.read().clone();
                            next.range = r;
                            spec.set(next);
                        }
                    },
                    for r in StatsRange::ALL {
                        option {
                            value: "{r.as_query()}",
                            selected: r == current.range,
                            "{r.label()}"
                        }
                    }
                }
            }

            div { class: "cb-field",
                label { r#for: "cb-breakdown", "Split" }
                select {
                    id: "cb-breakdown",
                    class: "cb-select",
                    // A split needs one measure, and one that belongs to a
                    // book rather than a sitting — so the control says why it
                    // is unavailable instead of silently doing nothing.
                    disabled: secondary.is_some() || !primary.supports_breakdown(),
                    onchange: move |e| {
                        let raw = e.value();
                        let mut next = spec.read().clone();
                        next.breakdown = if raw == "genre" {
                            ChartBreakdown::Genre
                        } else {
                            ChartBreakdown::None
                        };
                        spec.set(next);
                    },
                    option {
                        value: "none",
                        selected: current.breakdown == ChartBreakdown::None,
                        "{ChartBreakdown::None.label()}"
                    }
                    option {
                        value: "genre",
                        selected: current.breakdown == ChartBreakdown::Genre,
                        "{ChartBreakdown::Genre.label()}"
                    }
                }
                span { class: "cb-hint",
                    if secondary.is_some() {
                        "drop the comparison to split"
                    } else if !primary.supports_breakdown() {
                        "only per-book measures split"
                    } else {
                        "top genres, rest folded"
                    }
                }
            }
        }
    }
}

/// The chart itself, plus everything the result says about its own limits.
#[component]
fn ChartCanvas(
    result: Signal<Option<ChartResult>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
) -> Element {
    if let Some(msg) = error() {
        return rsx! {
            PageError { message: msg, back_to: Route::Stats {} }
        };
    }
    // Only the very first load blanks the surface; a spec change keeps the
    // previous chart on screen while its replacement is in flight, so the
    // page doesn't flash between every selection.
    if loading() && result.read().is_none() {
        return rsx! { PageLoading {} };
    }

    let Some(chart) = result.read().clone() else {
        return rsx! {};
    };

    rsx! {
        section {
            class: if loading() { "cb-canvas is-loading" } else { "cb-canvas" },
            "data-testid": "chart-canvas",
            if chart.is_empty() {
                p { class: "cb-empty", "data-testid": "chart-empty",
                    "Nothing recorded in this period yet. Try a wider period, "
                    "or a measure you've got data for."
                }
            } else {
                ChartPlot { result: chart.clone() }
                ChartLegend { result: chart.clone() }
                ChartTable { result: chart.clone() }
            }
            if chart.truncated {
                p { class: "cb-note", "data-testid": "chart-truncated",
                    "Showing the most recent {omnibus_shared::MAX_BUCKETS} periods — "
                    "the full range is longer than this axis can draw."
                }
            }
            for caveat in chart.caveats.iter() {
                p { class: "cb-note", "data-testid": "chart-caveat", "{caveat}" }
            }
        }
    }
}

/// A default measure for the impossible empty-selection state.
///
/// `ChartSpec::validate` rejects an empty measure list before it can reach the
/// server, and the picker never produces one — but the render path still has
/// to name something, and an `unwrap` in a production path is banned.
trait DefaultMeasure {
    fn unwrap_or_default_measure(self) -> ChartMeasure;
}

impl DefaultMeasure for Option<ChartMeasure> {
    fn unwrap_or_default_measure(self) -> ChartMeasure {
        self.unwrap_or(ChartMeasure::BooksFinished)
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
