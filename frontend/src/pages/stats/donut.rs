//! "Where the time went" — the window's genre mix as a pure-CSS
//! `conic-gradient` ring, with the read-vs-listened split beneath it, plus the
//! finished-book length rows the Finished drill-in draws. No charting library;
//! slice colors are accent-derived custom properties so every ring stays
//! theme-safe.

use dioxus::prelude::*;
use omnibus_shared::{GenreShare, StatsSummary};

/// Slices rendered before the rest folds into "Other".
const DONUT_SLICES: usize = 4;

/// Slice color tokens, defined in `atrium.css` off `--accent` / `--bg-3`.
const SLICE_VARS: [&str; DONUT_SLICES + 1] = [
    "var(--st-donut-c0)",
    "var(--st-donut-c1)",
    "var(--st-donut-c2)",
    "var(--st-donut-c3)",
    "var(--st-donut-other)",
];

/// Top-N shares plus an "Other" fold for the tail. Empty stays empty.
fn fold_shares(shares: &[GenreShare]) -> Vec<(String, i64)> {
    let mut folded: Vec<(String, i64)> = shares
        .iter()
        .take(DONUT_SLICES)
        .map(|s| (s.name.clone(), s.books))
        .collect();
    let rest: i64 = shares.iter().skip(DONUT_SLICES).map(|s| s.books).sum();
    if rest > 0 {
        folded.push(("Other".to_string(), rest));
    }
    folded
}

/// Integer percentages that always sum to exactly 100 (largest-remainder
/// rounding), so the ring closes and the legend never reads 99% or 101%.
pub(super) fn percentages(counts: &[i64]) -> Vec<i64> {
    let total: i64 = counts.iter().sum();
    if total <= 0 {
        return vec![0; counts.len()];
    }
    let mut floored: Vec<(usize, i64, i64)> = counts
        .iter()
        .enumerate()
        .map(|(i, &c)| (i, c * 100 / total, (c * 100) % total))
        .collect();
    let mut leftover = 100 - floored.iter().map(|(_, p, _)| p).sum::<i64>();
    // Hand the leftover points to the largest remainders, stable on ties.
    floored.sort_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(&b.0)));
    for slot in floored.iter_mut() {
        if leftover == 0 {
            break;
        }
        slot.1 += 1;
        leftover -= 1;
    }
    floored.sort_by_key(|(i, _, _)| *i);
    floored.into_iter().map(|(_, p, _)| p).collect()
}

/// The ring's `conic-gradient(...)` — **cumulative** stops per slice colour,
/// so a genre that rounds to nothing collapses to a zero-width slice rather
/// than leaving a stray hairline where the next colour starts.
fn donut_gradient(percents: &[i64]) -> String {
    let mut stops = Vec::with_capacity(percents.len());
    let mut at = 0;
    for (i, &p) in percents.iter().enumerate() {
        let var = SLICE_VARS[i.min(SLICE_VARS.len() - 1)];
        stops.push(format!("{var} {at}% {}%", at + p));
        at += p;
    }
    format!("conic-gradient({})", stops.join(", "))
}

/// The disclosure line for active books the ring can't describe.
fn untagged_note(untagged: i64) -> String {
    let noun = if untagged == 1 { "book" } else { "books" };
    format!("+{untagged} {noun} without a genre")
}

/// "Where the time went" — the genre ring with its legend, and the
/// read-vs-listened split beneath.
///
/// The centre reports `genre_tagged_books` — the population the slices are
/// drawn from — rather than `books_active`, which counts books the ring does
/// not describe. The difference is disclosed under the legend instead, where
/// it can't be read as a slice.
#[component]
pub(super) fn GenreDonut(summary: StatsSummary) -> Element {
    let folded = fold_shares(&summary.genre_share);
    let tagged = summary.genre_tagged_books;
    let untagged = (summary.books_active - tagged).max(0);
    let percents = percentages(&folded.iter().map(|(_, c)| *c).collect::<Vec<_>>());
    let gradient = donut_gradient(&percents);
    // Content-derived key: when a period switch lands a different mix, the
    // body remounts and replays the content-swap animation while the card
    // stays put. Same data → same key → no motion.
    let content_key = format!("{tagged}|{untagged}|{gradient}|{folded:?}");

    rsx! {
        div { class: "card st-spend", "data-testid": "stats-genre-donut",
            div { class: "label", "Where the time went" }
            if folded.is_empty() {
                p { class: "st-card-empty", "No tagged reading in this period yet." }
            } else {
                div { key: "{content_key}", class: "st-spend-body",
                    div {
                        class: "st-donut",
                        style: "background: {gradient};",
                        role: "img",
                        aria_label: "Genre share by book count",
                        div { class: "st-donut-hole",
                            div { class: "st-donut-count", "{tagged}" }
                            div { class: "st-donut-count-label", "tagged" }
                        }
                    }
                    ul { class: "st-donut-legend",
                        for (i, ((name, _), pct)) in folded.iter().zip(percents.iter()).enumerate() {
                            // Indexed, not name-keyed: a real genre named
                            // "Other" would otherwise collide with the
                            // synthetic fold-row above, and the parent's
                            // `content_key` already forces a full remount
                            // whenever the data itself changes.
                            li { key: "{i}", class: "st-donut-row",
                                span {
                                    class: "st-donut-swatch",
                                    style: "background: {SLICE_VARS[i.min(SLICE_VARS.len() - 1)]};",
                                }
                                span { class: "st-donut-name", "{name}" }
                                span { class: "st-donut-pct", "{pct}%" }
                            }
                        }
                    }
                }
                if untagged > 0 {
                    p { class: "st-card-note", "data-testid": "stats-donut-untagged",
                        {untagged_note(untagged)}
                    }
                }
            }
            FormatSplit { summary }
        }
    }
}

/// The read-vs-listened share of active seconds, along the card's foot. Two
/// bars rather than a caption: the ratio is the thing, and a reader takes it
/// off the bars without parsing two percentages.
#[component]
fn FormatSplit(summary: StatsSummary) -> Element {
    let total = summary.total_seconds();
    if total <= 0 {
        return rsx! {};
    }
    let percents = percentages(&[summary.reading_seconds, summary.listening_seconds]);
    rsx! {
        div { class: "st-split", "data-testid": "stats-format-split",
            for (label, pct, var) in [
                ("Read", percents[0], "var(--st-donut-c0)"),
                ("Listened", percents[1], "var(--st-donut-c1)"),
            ] {
                div { key: "{label}", class: "st-split-half",
                    div { class: "st-split-head",
                        span { class: "st-split-name", {label} }
                        // Keyed so a changed share fades the number in; the
                        // bar below animates its width via CSS transition
                        // instead (the row itself never remounts).
                        span { key: "{pct}", class: "st-split-pct", "{pct}%" }
                    }
                    div { class: "st-split-track",
                        div { class: "st-split-fill", style: "width: {pct}%; background: {var};" }
                    }
                }
            }
        }
    }
}

/// "How long they were" — the books finished in the window bucketed by page
/// count, on the same bar treatment as the split above.
///
/// Rows only, no card chrome: this is the Finished tile's detail, drawn inside
/// its drill-in rather than as a card of its own — a length distribution is a
/// fact *about* the books finished, not a peer of the count.
///
/// The server owns the buckets, their order, and their labels, so this renders
/// whatever it is handed — including the "Unknown" bucket, which is the point:
/// an audiobook has no page analogue, and dropping it would quietly report the
/// distribution over fewer books than the window actually holds.
#[component]
pub(super) fn LengthRows(summary: StatsSummary) -> Element {
    let buckets = summary.length_buckets;
    let total: i64 = buckets.iter().map(|b| b.books).sum();
    if total <= 0 {
        return rsx! {
            p { class: "st-card-empty", "data-testid": "stats-length-empty",
                "No books finished in this period yet."
            }
        };
    }
    let percents = percentages(&buckets.iter().map(|b| b.books).collect::<Vec<_>>());
    rsx! {
        div { class: "st-length-rows", "data-testid": "stats-length-split",
            for (i, (bucket, pct)) in buckets.iter().zip(percents.iter()).enumerate() {
                // Indexed rather than label-keyed: the bucket set is fixed and
                // server-owned, and two of the labels could in principle be
                // renamed to collide.
                div { key: "{i}", class: "st-split-half", "data-testid": "stats-length-row",
                    div { class: "st-split-head",
                        span { class: "st-split-name", "{bucket.label}" }
                        // The count, not the share: "3 books" answers the
                        // question a reader brought to a length chart, where
                        // "27%" needs the total to mean anything.
                        span { key: "{bucket.books}", class: "st-split-pct", "{bucket.books}" }
                    }
                    div { class: "st-split-track",
                        div {
                            class: "st-split-fill",
                            style: "width: {pct}%; background: {SLICE_VARS[i.min(SLICE_VARS.len() - 1)]};",
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
