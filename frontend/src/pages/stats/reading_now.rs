//! The standing band's two book lists: what the reader currently has open, and
//! what they most recently finished. Neither is windowed — a period switch
//! must not appear to change which books are on the go — so both sit outside
//! the switcher's reach.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{FinishedBook, MonthCount, ResumePoint, StatsSummary};

use super::goal::year_fraction;
use crate::components::{CoverTile, CoverTileKind};
use crate::{use_server_url, Route};

/// How far into the year the projection stays quiet. A handful of books over
/// the first fortnight extrapolates to a number nobody should read as a
/// forecast — one book by the 5th of January projects seventy-three.
const PROJECTION_MIN_FRACTION: f64 = 0.15;

/// Finished books so far this calendar year, taken off the trailing-12 series.
///
/// `books_per_month` always ends at the current month, so every elapsed month
/// of this year is in it; filtering on the year prefix is exact. Derived here
/// rather than read off `goal.current` because the projection is worth showing
/// to a reader who has set no goal at all.
fn books_this_year(months: &[MonthCount], year: &str) -> i64 {
    if year.is_empty() {
        return 0;
    }
    months
        .iter()
        .filter(|m| m.month.starts_with(year))
        .map(|m| m.books)
        .sum()
}

/// Where this year's pace lands by 31 December, or `None` when it is too early
/// in the year for the extrapolation to mean anything.
fn year_projection(summary: &StatsSummary) -> Option<i64> {
    let year = summary.as_of_day.get(..4)?;
    let fraction = year_fraction(&summary.as_of_day)?;
    if fraction < PROJECTION_MIN_FRACTION {
        return None;
    }
    let finished = books_this_year(&summary.books_per_month, year);
    if finished <= 0 {
        return None;
    }
    // Small counts over a 0..=1 fraction, far inside f64's exact range.
    #[allow(clippy::cast_precision_loss)]
    let projected = finished as f64 / fraction;
    #[allow(clippy::cast_possible_truncation)]
    let rounded = projected.round() as i64;
    Some(rounded)
}

/// The right-hand readout on an open book: how far in, in whatever unit that
/// book can answer.
///
/// An epub reports a whole-book percent; an audiobook reports none (its
/// position is a time offset), so it answers in chapters instead. A book that
/// can say neither says nothing rather than inventing a zero.
fn resume_readout(point: &ResumePoint) -> Option<String> {
    if let Some(pct) = point.record.progress_percent {
        return Some(format!("{pct}%"));
    }
    match (point.chapter_number, point.chapter_count) {
        (Some(n), Some(total)) => Some(format!("Ch {n} of {total}")),
        _ => None,
    }
}

/// Bar width for an open book — the same percent the readout states, and
/// nothing at all for a book that can't report one.
fn resume_percent(point: &ResumePoint) -> i64 {
    point.record.progress_percent.unwrap_or(0).clamp(0, 100)
}

/// "In progress" — the books currently open, with the year's projected finish
/// beneath them.
#[component]
pub(super) fn InProgressCard(books: Vec<ResumePoint>, summary: StatsSummary) -> Element {
    let server_url = use_server_url();
    let projection = year_projection(&summary);
    rsx! {
        div { class: "card st-open", "data-testid": "stats-in-progress",
            div { class: "label", "In progress" }
            if books.is_empty() {
                p { class: "st-card-empty", "Nothing open right now." }
            } else {
                div { class: "st-open-list",
                    for point in books.iter() {
                        {open_row(point, &server_url)}
                    }
                }
            }
            if let Some(projected) = projection {
                div { class: "st-open-foot", "data-testid": "stats-year-projection",
                    span { class: "st-open-foot-label", "At this pace you finish the year on" }
                    span { class: "st-open-foot-value", "{projected} books" }
                }
            }
        }
    }
}

/// One open book: cover, title and author, a progress bar, and the readout.
fn open_row(point: &ResumePoint, server_url: &str) -> Element {
    let uuid = point
        .book
        .unique_identifier
        .clone()
        .unwrap_or_else(|| point.record.book_uuid.clone());
    let title = point.book.display_title();
    let author = point.book.creators.first().map(|c| c.name.clone());
    let readout = resume_readout(point);
    let percent = resume_percent(point);
    rsx! {
        Link {
            key: "{uuid}",
            class: "st-open-row",
            to: Route::BookDetail { uuid: uuid.clone() },
            div { class: "st-open-cover",
                CoverTile {
                    book: point.book.clone(),
                    server_url: server_url.to_string(),
                    sizes: "44px".to_string(),
                    kind: CoverTileKind::ReadOnly,
                }
            }
            div { class: "st-open-body",
                div { class: "st-open-title", "{title}" }
                if let Some(author) = author {
                    div { class: "st-open-author", "{author}" }
                }
                div { class: "st-open-track",
                    div { class: "st-open-fill", style: "width: {percent}%" }
                }
            }
            if let Some(readout) = readout {
                div { class: "st-open-readout", "{readout}" }
            }
        }
    }
}

/// "Recently finished" — the most recent completions, with the rating each
/// carries.
///
/// Labelled by recency rather than by a count, so the card can never appear to
/// contradict the Finished tile above it: the tile counts a window, this lists
/// the latest whatever window is showing.
#[component]
pub(super) fn RecentlyFinishedCard(books: Vec<FinishedBook>) -> Element {
    let server_url = use_server_url();
    rsx! {
        div { class: "card st-finished", "data-testid": "stats-recently-finished",
            div { class: "label", "Recently finished" }
            if books.is_empty() {
                p { class: "st-card-empty", "Nothing finished yet." }
            } else {
                div { class: "st-finished-list",
                    for book in books.iter().take(4) {
                        {finished_row(book, &server_url)}
                    }
                }
            }
        }
    }
}

/// One finished book: cover, title and author, and its rating in stars.
fn finished_row(book: &FinishedBook, server_url: &str) -> Element {
    let ebook = super::drill_in::finished_book_as_ebook(book);
    rsx! {
        Link {
            key: "{book.book_uuid}",
            class: "st-finished-row",
            to: Route::BookDetail { uuid: book.book_uuid.clone() },
            div { class: "st-finished-cover",
                CoverTile {
                    book: ebook,
                    server_url: server_url.to_string(),
                    sizes: "34px".to_string(),
                    kind: CoverTileKind::ReadOnly,
                }
            }
            div { class: "st-finished-body",
                div { class: "st-finished-title", "{book.title}" }
                if let Some(author) = &book.author {
                    div { class: "st-finished-author", "{author}" }
                }
            }
            div { class: "st-finished-stars", {stars_label(book.rating)} }
        }
    }
}

/// A rating as filled and hollow stars, or the em-dash for an unrated book.
/// Half stars round to the nearer whole one — five glyphs cannot show a half,
/// and the drill-in's histogram is where the exact distribution lives.
fn stars_label(rating: Option<f64>) -> String {
    let Some(rating) = rating else {
        return "\u{2014}".to_string();
    };
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let filled = rating.round().clamp(0.0, 5.0) as usize;
    format!(
        "{}{}",
        "\u{2605}".repeat(filled),
        "\u{2606}".repeat(5 - filled)
    )
}

#[cfg(test)]
mod tests;
