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

mod notes;
mod plot;

use notes::ChartNotes;
use plot::{ChartLegend, ChartPlot, ChartTable};

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
/// The picker. Each control writes one field of the spec; the effect above
/// turns any write into a refetch.
///
/// Measures are a checkbox group rather than a fixed pair of dropdowns,
/// because the real constraint is the number of **scales** a chart can label,
/// not the number of measures: any number sharing a unit sit on one axis and
/// stay directly comparable. A measure is greyed out only once both scales are
/// claimed by other units — and by the same `ChartSpec::can_add` the server
/// validates with, so a control that looks available can never produce a spec
/// the server then rejects.
#[component]
fn ChartControls(spec: Signal<ChartSpec>) -> Element {
    let current = spec.read().clone();
    let units = current.units();
    // A split describes one measure's population, so it needs exactly one.
    let solo = (current.measures.len() == 1)
        .then(|| current.measures.first().copied())
        .flatten();

    rsx! {
        section { class: "cb-controls", "data-testid": "chart-controls",
            fieldset { class: "cb-measures", "data-testid": "chart-measures",
                legend { "Measures" }
                div { class: "cb-measure-list",
                    for m in ChartMeasure::ALL {
                        {
                            let on = current.measures.contains(&m);
                            // The last remaining measure can't be removed —
                            // an empty chart is not a state the picker should
                            // be able to reach.
                            let last = on && current.measures.len() == 1;
                            let blocked = !on && !current.can_add(m);
                            rsx! {
                                div {
                                    class: if on { "cb-measure is-on" } else { "cb-measure" },
                                    input {
                                        id: "cb-m-{m.as_query()}",
                                        r#type: "checkbox",
                                        checked: on,
                                        disabled: last || blocked,
                                        onchange: move |_| {
                                            let mut next = spec.read().clone();
                                            next.toggle(m);
                                            // A split survives only while it
                                            // still has its one splittable
                                            // measure to describe.
                                            let keeps_split = next.measures.len() == 1
                                                && next
                                                    .measures
                                                    .first()
                                                    .is_some_and(|f| f.supports_breakdown());
                                            if !keeps_split {
                                                next.breakdown = ChartBreakdown::None;
                                            }
                                            spec.set(next);
                                        },
                                    }
                                    label { r#for: "cb-m-{m.as_query()}", "{m.label()}" }
                                    span { class: "cb-measure-meta",
                                        "{m.unit().label()} · {m.grain().label()}"
                                    }
                                }
                            }
                        }
                    }
                }
                p { class: "cb-hint", "data-testid": "chart-scales",
                    if units.len() < omnibus_shared::MAX_AXES {
                        "One scale in use — anything else can still join."
                    } else {
                        "Both scales in use. Measures in other units are greyed out until you free one."
                    }
                }
            }

            div { class: "cb-fields",
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
                        // book — so the control says why it is unavailable
                        // instead of silently doing nothing.
                        disabled: !solo.is_some_and(|m| m.supports_breakdown()),
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
                        match solo {
                            None => "one measure only",
                            Some(m) if !m.supports_breakdown() => "only per-book measures split",
                            Some(_) => "top genres, rest folded",
                        }
                    }
                }
            }
        }
    }
}

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
            // The caveats and the truncation note live inside the notes now,
            // under the heading that gives them their context — a bare line
            // under a chart reads as a disclaimer nobody asked for.
            ChartNotes { result: chart.clone() }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
