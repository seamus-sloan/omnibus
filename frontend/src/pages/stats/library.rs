//! The Library scope's hero: how big the collection is, in words, pages, and
//! hours of audio. Describes the library rather than the reader, so the period
//! switcher cannot reach it. Every figure renders the coverage behind it — a
//! bare total would report a partly-backfilled library as a smaller one with
//! total confidence.

use dioxus::prelude::*;
use omnibus_shared::{LibrarySize, MeasuredTotal};

use super::group_thousands;
use crate::format::plural_noun;

/// One rendered figure: the total, its unit, and the coverage line beneath.
struct Figure {
    value: String,
    unit: &'static str,
    coverage: String,
}

/// A large count in the form a reader can hold — "412M", "1.6M", "94.2K",
/// "812". Thousands separators are honest but unreadable at this scale, and
/// nobody needs the last four digits of a word count.
fn compact(n: i64) -> String {
    // Totals sit far below f64's 2^52 exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    let v = n as f64;
    // Each tier opens at 999.5 of the one below rather than at a clean power
    // of ten: 999_999 rounds to 1000 at "K", so it has to render as "1.0M".
    for (limit, div, suffix) in [(999.5e6, 1e9, "B"), (999.5e3, 1e6, "M"), (1e4, 1e3, "K")] {
        if v >= limit {
            let scaled = v / div;
            let digits = usize::from(scaled < 100.0);
            return format!("{scaled:.digits$}{suffix}");
        }
    }
    n.to_string()
}

/// Audio length in the unit that fits it: hours below a week, days beyond.
/// "94 days of audio" is the sentence this card exists to let a reader say.
fn audio_value(seconds: i64) -> (String, &'static str) {
    // Well inside f64's exact-integer range; display only.
    #[allow(clippy::cast_precision_loss)]
    let hours = seconds as f64 / 3600.0;
    // Round first, then pick the unit off the rounded figure: branching on the
    // raw hours renders 1h40m as "2 hour", and promotes to days only after the
    // hours reading has already rounded to 168.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let whole_hours = hours.round() as i64;
    if whole_hours < 168 {
        return (
            whole_hours.to_string(),
            if whole_hours == 1 { "hour" } else { "hours" },
        );
    }
    (format!("{:.0}", hours / 24.0), "days")
}

/// "across 1,204 of 1,510 books" — the denominator, always. A figure without
/// it is a guess wearing a number.
fn coverage(measured: &MeasuredTotal, library_books: i64) -> String {
    format!(
        "across {} of {} {}",
        group_thousands(measured.books),
        group_thousands(library_books),
        plural_noun(library_books, "book")
    )
}

/// The three figures the library supports, skipping any nothing has been
/// measured for — a "0 words" tile describes a library that doesn't exist.
fn build_figures(size: &LibrarySize) -> Vec<Figure> {
    let mut figures = Vec::with_capacity(3);
    if !size.words.is_empty() {
        figures.push(Figure {
            value: compact(size.words.total),
            unit: "words",
            coverage: coverage(&size.words, size.books),
        });
    }
    if !size.pages.is_empty() {
        figures.push(Figure {
            value: compact(size.pages.total),
            unit: "est. pages",
            coverage: coverage(&size.pages, size.books),
        });
    }
    if !size.listening_seconds.is_empty() {
        let (value, unit) = audio_value(size.listening_seconds.total);
        figures.push(Figure {
            value,
            unit,
            coverage: coverage(&size.listening_seconds, size.books),
        });
    }
    figures
}

/// A count spelled the way a reader would say it — "412 million", "1.6
/// million", "94 thousand", "812". The hero reads as a sentence, and "412M"
/// mid-sentence reads as a part number.
fn spelled(n: i64) -> String {
    // Totals sit far below f64's 2^52 exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    let v = n as f64;
    // Each tier opens at 999.5 of the one below rather than at a clean power
    // of ten: 999,999 rounds to 1000 at "thousand", so it has to promote.
    for (limit, div, word) in [
        (999.5e6, 1e9, "billion"),
        (999.5e3, 1e6, "million"),
        (1e4, 1e3, "thousand"),
    ] {
        if v >= limit {
            let scaled = v / div;
            let digits = usize::from(scaled < 100.0);
            return format!("{scaled:.digits$} {word}");
        }
    }
    group_thousands(n)
}

/// The hero's opening sentence, split so the middle clause can be tinted.
///
/// Deliberately says nothing about how long the shelf would take to read:
/// that answer is the *reader's* pace, and this scope is labelled "whole shelf
/// · not period-scoped". Borrowing a user figure to finish the sentence would
/// contradict the boundary the page just drew.
fn hero_sentence(size: &LibrarySize) -> (String, Option<String>, &'static str) {
    let books = format!(
        "{} {}.",
        group_thousands(size.books),
        plural_noun(size.books, "book")
    );
    if size.words.is_empty() {
        return (books, None, "");
    }
    (
        books,
        Some(format!("{} words", spelled(size.words.total))),
        " waiting on the shelf.",
    )
}

/// The Library scope's hero: the collection in one sentence, over its size in
/// words, pages, and hours of audio.
///
/// Renders nothing at all until the fetch lands or when the library has been
/// measured for nothing: three zeroes read as a claim about the collection
/// rather than about the backfill.
#[component]
pub(super) fn LibrarySizeHero(size: Option<LibrarySize>) -> Element {
    let Some(size) = size else {
        return rsx! {};
    };
    let figures = build_figures(&size);
    if figures.is_empty() {
        return rsx! {};
    }
    let (books, words, tail) = hero_sentence(&size);
    rsx! {
        div { class: "card st-lib", "data-testid": "stats-library-size",
            p { class: "st-lib-sentence",
                "{books} "
                if let Some(words) = words {
                    span { class: "st-lib-accent", "{words}" }
                    "{tail}"
                }
            }
            div { class: "st-lib-grid",
                for figure in figures.iter() {
                    div { key: "{figure.unit}", class: "st-lib-figure", "data-testid": "stats-library-figure",
                        div { class: "st-lib-value",
                            {figure.value.clone()}
                            span { class: "st-lib-unit", " {figure.unit}" }
                        }
                        div { class: "st-lib-coverage", "{figure.coverage}" }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
