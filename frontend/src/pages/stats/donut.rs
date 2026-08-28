//! Period-scoped composition cards: the genre donut (a pure-CSS
//! `conic-gradient` ring of genre share by distinct book count) and the
//! read-vs-listened format split. No charting library — slice colors are
//! accent-derived CSS custom properties so both stay theme-safe.

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
fn percentages(counts: &[i64]) -> Vec<i64> {
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

/// The ring's `conic-gradient(...)` — cumulative stops per slice color.
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

/// "What you read" — the genre-share donut with center book count and a
/// `label · %` legend. Empty window → a friendly note instead of a ring.
///
/// The center reports `genre_tagged_books`, the population the slices are
/// drawn from, rather than `books_active`: a book with no genre reaches the
/// latter but no slice, so using it put "12 books" around a ring that
/// described four. Untagged reading is disclosed under the legend instead,
/// where it can't be read as part of the ring.
#[component]
pub(super) fn GenreDonut(summary: StatsSummary) -> Element {
    let folded = fold_shares(&summary.genre_share);
    let tagged = summary.genre_tagged_books;
    let untagged = (summary.books_active - tagged).max(0);
    if folded.is_empty() {
        return rsx! {
            div { class: "card st-donut-card", "data-testid": "stats-genre-donut",
                div { class: "label", "What you read" }
                p { class: "st-donut-empty", "No tagged reading in this period yet." }
            }
        };
    }
    let percents = percentages(&folded.iter().map(|(_, c)| *c).collect::<Vec<_>>());
    let gradient = donut_gradient(&percents);
    // Content-derived key: when a period switch lands a different mix, the
    // body remounts and replays the content-swap animation while the card
    // stays put. Same data → same key → no motion.
    let content_key = format!("{tagged}|{untagged}|{gradient}|{folded:?}");
    rsx! {
        div { class: "card st-donut-card", "data-testid": "stats-genre-donut",
            div { class: "label", "What you read" }
            div { key: "{content_key}", class: "st-donut-body",
                div { class: "st-donut", style: "background: {gradient};", role: "img",
                    aria_label: "Genre share by book count",
                    div { class: "st-donut-hole",
                        div { class: "st-donut-count", "{tagged}" }
                        div { class: "st-donut-count-label mono", "tagged" }
                    }
                }
                ul { class: "st-donut-legend",
                    for (i, ((name, _), pct)) in folded.iter().zip(percents.iter()).enumerate() {
                        // Indexed, not name-keyed: a real genre named "Other"
                        // would otherwise collide with the synthetic fold-row
                        // above, and the parent's `content_key` already forces
                        // a full remount whenever the data itself changes.
                        li { key: "{i}", class: "st-donut-row",
                            span {
                                class: "st-donut-swatch",
                                style: "background: {SLICE_VARS[i.min(SLICE_VARS.len() - 1)]};",
                            }
                            span { class: "st-donut-name", "{name}" }
                            span { class: "st-donut-pct mono", "{pct}%" }
                        }
                    }
                }
            }
            if untagged > 0 {
                p { class: "st-donut-untagged mono", "data-testid": "stats-donut-untagged",
                    {untagged_note(untagged)}
                }
            }
        }
    }
}

/// The disclosure line for active books the ring can't describe.
fn untagged_note(untagged: i64) -> String {
    let noun = if untagged == 1 { "book" } else { "books" };
    format!("+{untagged} {noun} without a genre")
}

/// "How you consumed them" — reading vs listening share of active seconds.
#[component]
pub(super) fn FormatSplit(summary: StatsSummary) -> Element {
    let total = summary.total_seconds();
    if total <= 0 {
        return rsx! {
            div { class: "card st-format-card", "data-testid": "stats-format-split",
                div { class: "label", "How you consumed them" }
                p { class: "st-donut-empty", "No activity in this period yet." }
            }
        };
    }
    let percents = percentages(&[summary.reading_seconds, summary.listening_seconds]);
    rsx! {
        div { class: "card st-format-card", "data-testid": "stats-format-split",
            div { class: "label", "How you consumed them" }
            for (label, pct, var) in [
                ("Read", percents[0], "var(--st-donut-c0)"),
                ("Listened", percents[1], "var(--st-donut-c1)"),
            ] {
                div { class: "st-format-row",
                    div { class: "st-format-row-head",
                        span { class: "st-format-name", {label} }
                        // Keyed so a changed share fades the number in; the
                        // bar below animates its width via CSS transition
                        // instead (the row itself never remounts).
                        span { key: "{pct}", class: "st-format-pct mono", "{pct}%" }
                    }
                    div { class: "st-format-track",
                        div { class: "st-format-fill", style: "width: {pct}%; background: {var};" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn share(name: &str, books: i64) -> GenreShare {
        GenreShare {
            name: name.into(),
            books,
        }
    }

    #[test]
    fn fold_shares_keeps_top_four_and_folds_the_tail_into_other() {
        let shares: Vec<GenreShare> = (1..=6).map(|i| share(&format!("g{i}"), 7 - i)).collect();
        let folded = fold_shares(&shares);
        assert_eq!(folded.len(), 5);
        assert_eq!(folded[0], ("g1".to_string(), 6));
        assert_eq!(folded[4], ("Other".to_string(), 3));

        assert!(fold_shares(&[]).is_empty());
        assert_eq!(fold_shares(&[share("solo", 2)]).len(), 1);
    }

    #[test]
    fn percentages_always_sum_to_one_hundred() {
        for counts in [
            vec![1, 1, 1],
            vec![2, 1],
            vec![7, 2, 1],
            vec![1, 1, 1, 1, 1, 1, 1],
        ] {
            let p = percentages(&counts);
            assert_eq!(p.iter().sum::<i64>(), 100, "counts {counts:?} → {p:?}");
        }
        assert_eq!(percentages(&[3, 1]), vec![75, 25]);
        assert_eq!(percentages(&[]), Vec::<i64>::new());
        assert_eq!(percentages(&[0, 0]), vec![0, 0]);
    }

    #[test]
    fn untagged_note_singularizes_one_book() {
        assert_eq!(untagged_note(1), "+1 book without a genre");
        assert_eq!(untagged_note(4), "+4 books without a genre");
    }

    #[test]
    fn fold_shares_other_covers_the_whole_tail_the_server_sent() {
        // The server no longer truncates to a top-8, so "Other" is the real
        // remainder rather than ranks five through eight.
        let shares: Vec<GenreShare> = (1..=12).map(|i| share(&format!("g{i}"), 1)).collect();
        let folded = fold_shares(&shares);
        assert_eq!(folded.len(), 5);
        assert_eq!(folded[4], ("Other".to_string(), 8));
        let pct = percentages(&folded.iter().map(|(_, c)| *c).collect::<Vec<_>>());
        assert_eq!(pct.iter().sum::<i64>(), 100);
    }

    #[test]
    fn donut_gradient_builds_cumulative_stops() {
        let g = donut_gradient(&[36, 28, 20, 16]);
        assert_eq!(
            g,
            "conic-gradient(var(--st-donut-c0) 0% 36%, var(--st-donut-c1) 36% 64%, \
             var(--st-donut-c2) 64% 84%, var(--st-donut-c3) 84% 100%)"
        );
    }
}
