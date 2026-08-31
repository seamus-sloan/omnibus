//! The notes under the chart: what it shows, how the scales work, why some
//! measures are unavailable, and what the numbers can't tell you.
//!
//! Written from the **live result**, not from static copy. A reader who has
//! just been stopped by a greyed-out checkbox wants to know why *that* one is
//! greyed, and boilerplate that described the feature in general would leave
//! them to work it out — so every sentence here names the units, measures and
//! genres actually on screen.
//!
//! Sentences live next to the thing they describe: a measure's own line comes
//! from `ChartMeasure::description`, so changing what a measure counts and
//! changing how it is explained is one edit.

use dioxus::prelude::*;
use omnibus_shared::{ChartMeasure, ChartResult, ChartUnit, BREAKDOWN_LIMIT, MAX_AXES};

/// Join unit names into "books and pages" / "books, pages and minutes".
fn join_units(units: &[ChartUnit]) -> String {
    let names: Vec<&str> = units.iter().map(ChartUnit::label).collect();
    match names.as_slice() {
        [] => String::new(),
        [a] => (*a).to_string(),
        [a, b] => format!("{a} and {b}"),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// How the y-scales are laid out, in one sentence.
///
/// The two-axis case says outright that a crossing means nothing. It is the
/// single easiest thing to misread on this chart, and a reader who has not
/// been told will read a relationship into it.
fn scales_note(result: &ChartResult) -> Option<String> {
    let units: Vec<ChartUnit> = result.axes.iter().map(|a| a.unit).collect();
    match units.as_slice() {
        [] => None,
        [only] => Some(format!(
            "Everything here is measured in {}, so it all shares one scale and \
             the heights are directly comparable.",
            only.label()
        )),
        [left, right, ..] => Some(format!(
            "Two units, so two scales — {} on the left, {} on the right. They \
             are sized independently, so where one series crosses another \
             means nothing.",
            left.label(),
            right.label()
        )),
    }
}

/// Why the picker has closed off part of its list, when it has.
fn availability_note(result: &ChartResult) -> String {
    let units: Vec<ChartUnit> = result.axes.iter().map(|a| a.unit).collect();
    if units.len() < MAX_AXES {
        return "One scale is still free, so anything in the list can join.".to_string();
    }
    format!(
        "A chart can label two scales, and {} have both. Anything else in \
         those two units can still join; every other unit is greyed out until \
         you clear a measure and free a scale.",
        join_units(&units)
    )
}

/// What a split does to the marks, when one is on.
fn split_note(result: &ChartResult) -> Option<String> {
    let slices = result.series.iter().filter(|s| s.slice.is_some()).count();
    if slices == 0 {
        return None;
    }
    let folded = result
        .series
        .iter()
        .any(|s| s.slice.as_deref() == Some(omnibus_shared::OTHER_LABEL));
    let tail = if folded {
        format!(
            " Only the {BREAKDOWN_LIMIT} genres you read most get their own \
             colour; the rest are folded into Other."
        )
    } else {
        String::new()
    };
    Some(if result.stacked {
        format!(
            "Split by genre, and the slices stack — each column is that \
             period's total, cut up by genre.{tail}"
        )
    } else {
        format!(
            "Split by genre, side by side rather than stacked: these are \
             averages, and averages don't add up into a total.{tail}"
        )
    })
}

/// What dragging across the chart does, and the limit of what it can do.
///
/// Stated rather than left to be discovered: a zoom that silently refuses to
/// subdivide looks broken, where one that says it only narrows what was
/// already fetched is simply honest about being a client-side view.
fn zoom_note(result: &ChartResult) -> String {
    let base = "Drag across the chart to zoom into a stretch of it.".to_string();
    if result.truncated {
        return format!(
            "{base} It narrows the periods already loaded — it can't reach \
             past the ones this axis had to clip, and it won't break a period \
             into smaller ones."
        );
    }
    format!(
        "{base} It narrows the periods already loaded rather than fetching \
         finer ones, so zooming into three months shows three periods, not \
         ninety days — change Group by for that."
    )
}

/// What an empty bucket means, which differs by aggregate and is the other
/// easy misreading on this chart.
fn empty_note(result: &ChartResult) -> Option<String> {
    let has_average = result
        .series
        .iter()
        .any(|s| s.measure.aggregate() == omnibus_shared::ChartAggregate::Average);
    let has_total = result
        .series
        .iter()
        .any(|s| s.measure.aggregate() != omnibus_shared::ChartAggregate::Average);
    match (has_total, has_average) {
        (true, true) => Some(
            "A quiet period reads as zero for the totals, but the averages go \
             blank and the line breaks — you can't average nothing."
                .to_string(),
        ),
        (false, true) => Some(
            "Where nothing was recorded the line breaks rather than dropping \
             to zero — you can't average nothing."
                .to_string(),
        ),
        _ => None,
    }
}

/// The notes panel.
#[component]
pub fn ChartNotes(result: ReadSignal<ChartResult>) -> Element {
    let result = result.read();
    if result.series.is_empty() {
        return rsx! {};
    }
    // Deduplicated: a split renders one series per slice, all of the same
    // measure, and repeating its sentence six times would be noise.
    let mut measures: Vec<ChartMeasure> = Vec::new();
    for s in result.series.iter() {
        if !measures.contains(&s.measure) {
            measures.push(s.measure);
        }
    }

    rsx! {
        details { class: "cb-notes", "data-testid": "chart-notes",
            summary { class: "cb-notes-toggle", "What this chart shows" }
            div { class: "cb-notes-body",
                dl { class: "cb-notes-measures",
                    for m in measures.iter() {
                        dt { class: "cb-notes-term", "{m.label()}" }
                        dd { class: "cb-notes-def",
                            "{m.description()} Measured {m.grain().label()}, in {m.unit().label()}."
                        }
                    }
                }
                if let Some(note) = scales_note(&result) {
                    p { class: "cb-notes-line", "data-testid": "chart-notes-scales", "{note}" }
                }
                p { class: "cb-notes-line", "data-testid": "chart-notes-availability",
                    "{availability_note(&result)}"
                }
                if let Some(note) = split_note(&result) {
                    p { class: "cb-notes-line", "{note}" }
                }
                if let Some(note) = empty_note(&result) {
                    p { class: "cb-notes-line", "{note}" }
                }
                p { class: "cb-notes-line", "data-testid": "chart-notes-zoom",
                    "{zoom_note(&result)}"
                }
                if !result.caveats.is_empty() {
                    p { class: "cb-notes-head", "What these numbers can't tell you" }
                    ul { class: "cb-notes-caveats",
                        for caveat in result.caveats.iter() {
                            li { "data-testid": "chart-caveat", "{caveat}" }
                        }
                    }
                }
                if result.truncated {
                    p { class: "cb-notes-line", "data-testid": "chart-truncated",
                        "The range runs longer than this axis can draw, so only \
                         the most recent {omnibus_shared::MAX_BUCKETS} periods are shown."
                    }
                }
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
