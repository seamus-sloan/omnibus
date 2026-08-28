//! Library-composition card: what the collection is made of, by format,
//! language, publisher, publication decade, and genre. Sits in the all-time
//! section beside the library-size card and never moves with the switcher.
//! Titled apart from `donut.rs`'s period-scoped "How you consumed them", which
//! is read-vs-listened *seconds*, not the shelf's own mix.

use dioxus::prelude::*;
use omnibus_shared::{CompositionDimension, CompositionSlice, LibraryComposition};

use super::group_thousands;

/// One rendered dimension: its heading, its bars, and the line beneath them
/// that says what the bars can't speak for.
#[derive(Clone, PartialEq)]
struct Panel {
    title: &'static str,
    testid: &'static str,
    slices: Vec<CompositionSlice>,
    note: Option<String>,
    empty: &'static str,
}

/// Bar width as a percentage of the dimension's largest slice.
///
/// Scaled to the tallest bar rather than to the library total: a histogram
/// whose bars are all four pixels wide because one bucket dominates has drawn
/// the shape out of itself.
fn bar_width(books: i64, max: i64) -> i64 {
    if max <= 0 {
        return 0;
    }
    (books * 100 / max).clamp(0, 100)
}

/// "across 1,204 of 1,510 books" — the denominator, always. The same sentence
/// the library-size card uses, for the same reason: a distribution without its
/// coverage is a guess wearing a chart.
fn coverage_note(dim: &CompositionDimension, library_books: i64) -> String {
    format!(
        "across {} of {} books",
        group_thousands(dim.coverage.books),
        group_thousands(library_books)
    )
}

/// The format panel's disclosure: how many books are held in more than one
/// format, and so are counted in more than one bar.
///
/// Without it the bars simply don't add up to the library and a reader has no
/// way to tell whether that is double-counting or a missing bucket.
fn overlap_note(dim: &CompositionDimension) -> Option<String> {
    let overlap = dim.overlap();
    if overlap == 0 {
        return None;
    }
    let noun = if overlap == 1 { "book" } else { "books" };
    Some(format!("+{overlap} {noun} held in more than one format"))
}

/// The card's footnote for `books` rows whose files are gone. They carry no
/// format at all, so they'd otherwise vanish from the format bars and leave
/// the counts quietly failing to reconcile against the library.
fn ghosted_note(ghosted: i64) -> Option<String> {
    if ghosted == 0 {
        return None;
    }
    let noun = if ghosted == 1 { "book" } else { "books" };
    Some(format!(
        "{ghosted} {noun} excluded — indexed once, no files on disk now"
    ))
}

/// The five panels, in the order they read: what the files are, then what the
/// books are.
fn build_panels(c: &LibraryComposition) -> Vec<Panel> {
    vec![
        Panel {
            title: "Formats",
            testid: "stats-composition-formats",
            slices: c.formats.slices.clone(),
            // Coverage is always the whole library here (a live book has a
            // file by definition), so the useful disclosure is the overlap.
            note: overlap_note(&c.formats),
            empty: "No files indexed yet.",
        },
        Panel {
            title: "Languages",
            testid: "stats-composition-languages",
            slices: c.languages.slices.clone(),
            note: Some(coverage_note(&c.languages, c.books)),
            empty: "No language metadata yet.",
        },
        Panel {
            title: "Publishers",
            testid: "stats-composition-publishers",
            slices: c.publishers.slices.clone(),
            note: Some(coverage_note(&c.publishers, c.books)),
            empty: "No publisher metadata yet.",
        },
        Panel {
            title: "Published",
            testid: "stats-composition-decades",
            slices: c.decades.slices.clone(),
            // The uncovered books here are the ones with an absent or
            // unparseable `pubdate` — reported as unknown, never bucketed
            // into a decade they'd share with real dates.
            note: Some(coverage_note(&c.decades, c.books)),
            empty: "No publication dates yet.",
        },
        Panel {
            title: "Genres",
            testid: "stats-composition-genres",
            slices: c.genres.slices.clone(),
            note: Some(format!(
                "hand-assigned \u{2014} {}",
                coverage_note(&c.genres, c.books)
            )),
            empty: "No genres assigned yet.",
        },
    ]
}

/// "What your library is made of" — format, language, publisher, publication
/// decade, and genre.
///
/// Renders nothing at all until the fetch lands, or for a library with no live
/// books: five empty panels describe a collection that doesn't exist.
#[component]
pub(super) fn LibraryCompositionCard(composition: Option<LibraryComposition>) -> Element {
    let Some(composition) = composition else {
        return rsx! {};
    };
    if composition.is_empty() {
        return rsx! {};
    }
    let panels = build_panels(&composition);
    let ghosted = ghosted_note(composition.ghosted_books);
    rsx! {
        div { class: "card st-comp-card", "data-testid": "stats-library-composition",
            div { class: "label", "What your library is made of" }
            div { class: "st-comp-grid",
                for panel in panels.iter() {
                    CompositionPanel { key: "{panel.testid}", panel: panel.clone() }
                }
            }
            if let Some(note) = ghosted {
                p { class: "mono st-comp-ghosted", "data-testid": "stats-composition-ghosted",
                    "{note}"
                }
            }
        }
    }
}

/// One dimension's bars, or its empty state. A dimension nothing in the
/// library carries renders a sentence rather than an axis with no bars on it.
#[component]
fn CompositionPanel(panel: Panel) -> Element {
    let guard = &panel;
    let max = guard.slices.iter().map(|s| s.books).max().unwrap_or(0);
    rsx! {
        section { class: "st-comp-panel", "data-testid": "{guard.testid}",
            h4 { class: "st-comp-title", "{guard.title}" }
            if guard.slices.is_empty() {
                p { class: "st-donut-empty", "data-testid": "stats-composition-empty",
                    "{guard.empty}"
                }
            } else {
                div { class: "st-length-rows",
                    for (i, slice) in guard.slices.iter().enumerate() {
                        // Indexed rather than label-keyed: the server folds a
                        // tail into a synthetic "Other" row that a real
                        // publisher of that name could collide with.
                        div { key: "{i}", class: "st-format-row", "data-testid": "stats-composition-bar",
                            div { class: "st-format-row-head",
                                span { class: "st-format-name", "{slice.label}" }
                                // The count, not the share: "48 books" answers
                                // the question a reader brought to a
                                // composition chart. Grouped like every other
                                // figure on the card, so a four-digit bucket
                                // doesn't read differently from its own note.
                                span { class: "mono st-format-pct", "{group_thousands(slice.books)}" }
                            }
                            div { class: "st-format-track",
                                div {
                                    class: "st-format-fill",
                                    style: "width: {bar_width(slice.books, max)}%; background: var(--st-donut-c0);",
                                }
                            }
                        }
                    }
                }
                if let Some(note) = guard.note.clone() {
                    p { class: "mono st-comp-note", "data-testid": "stats-composition-note",
                        "{note}"
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
