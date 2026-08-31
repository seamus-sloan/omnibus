//! "The standouts" — the windowed band's grid of single most-X figures, one
//! card per superlative. Every row is conditional, so the grid's own length
//! reports how much the window holds — see [`build_rows`] for which fields
//! feed it.

use dioxus::prelude::*;
use omnibus_shared::{BookSuperlative, RankedEntity, StatsSummary, FASTEST_READ_MIN_SECS};

use super::heatmap::{civil_from_days, day_number, format_active_time, month_abbr};

/// One rendered superlative: what it measures, what won, and by how much.
struct Row {
    label: &'static str,
    headline: String,
    detail: String,
}

/// A UTC `YYYY-MM-DD` as "14 Nov 2023", falling back to the raw string when
/// it can't be parsed — a malformed day is still better company for its figure
/// than no label at all.
fn pretty_day(day: &str) -> String {
    let Some((y, m, d)) = day_number(day).map(civil_from_days) else {
        return day.to_string();
    };
    format!("{d} {} {y}", month_abbr(m))
}

/// "412 pages" / "1 page" — the unit belongs to the row, not to
/// `BookSuperlative::value`, which carries a bare number.
fn pages_detail(pages: i64) -> String {
    let plural = if pages == 1 { "" } else { "s" };
    format!("{pages} page{plural}")
}

/// "in 3 days" / "in a day" — a one-day read reads better named than
/// numbered, and the server already collapses a same-day read to 1.
fn days_detail(days: i64) -> String {
    if days == 1 {
        return "in a day".to_string();
    }
    format!("in {days} days")
}

/// A book row, or nothing when the server omitted that superlative.
fn book_row(
    label: &'static str,
    book: Option<&BookSuperlative>,
    detail: impl Fn(i64) -> String,
) -> Option<Row> {
    let book = book?;
    Some(Row {
        label,
        headline: book.title.clone(),
        detail: detail(book.value),
    })
}

/// A ranked-entity row (most-read author / subject) off the top of a list the
/// server already sends. Absent for an empty list, and for a zero-second
/// leader — a name with no time behind it isn't a superlative.
fn ranked_row(label: &'static str, ranked: &[RankedEntity]) -> Option<Row> {
    let top = ranked.first().filter(|r| r.seconds > 0)?;
    Some(Row {
        label,
        headline: top.name.clone(),
        detail: format_active_time(top.seconds),
    })
}

/// Every superlative the window supports, in reading order. Book-length rows
/// first (they're what a reader quotes), then the time-shaped ones, then the
/// two rankings.
///
/// The last three come off fields the payload has always carried and no web
/// surface drew: the busiest week, and the top-ranked author and subject.
fn build_rows(summary: &StatsSummary) -> Vec<Row> {
    let s = &summary.superlatives;
    let busiest_week = summary
        .busiest_week_start
        .as_deref()
        .filter(|_| summary.busiest_week_seconds > 0)
        .map(|day| Row {
            label: "Busiest week",
            headline: format!("Week of {}", pretty_day(day)),
            detail: format_active_time(summary.busiest_week_seconds),
        });
    [
        book_row("Longest book", s.longest_book.as_ref(), pages_detail),
        book_row("Shortest book", s.shortest_book.as_ref(), pages_detail),
        book_row("Fastest read", s.fastest_read.as_ref(), days_detail),
        book_row(
            "Longest sitting",
            s.longest_sit.as_ref(),
            format_active_time,
        ),
        s.biggest_day.as_ref().map(|d| Row {
            label: "Biggest day",
            headline: pretty_day(&d.day),
            detail: format_active_time(d.seconds),
        }),
        busiest_week,
        ranked_row("Most-read author", &summary.top_authors),
        ranked_row("Most-read subject", &summary.top_tags),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The fastest-read caveat, stated in whatever unit the floor is set to so
/// the copy can't drift from `FASTEST_READ_MIN_SECS`.
fn fastest_read_note() -> String {
    format!(
        "Fastest read counts days from your first tracked session, over books with at least \
         {} of recorded time \u{2014} reading done before tracking, or on a device that reports \
         nothing, can only make a book look faster than it was.",
        format_active_time(FASTEST_READ_MIN_SECS)
    )
}

/// "The standouts" — the window's superlatives as a card grid, or nothing at
/// all.
///
/// The grid is omitted rather than emptied: a heading over a friendly "no
/// standouts yet" line is a row of furniture on a page that already has an
/// empty state one level up.
///
/// The gate is this assembled list, **not** [`omnibus_shared::Superlatives::is_empty`]
/// — the busiest week and the two rankings come off fields outside that
/// struct, so a window with only a busiest week is `is_empty()` and still has
/// something to show.
#[component]
pub(super) fn StandoutsGrid(summary: StatsSummary) -> Element {
    let rows = build_rows(&summary);
    if rows.is_empty() {
        return rsx! {};
    }
    let show_note = summary.superlatives.fastest_read.is_some();
    rsx! {
        div { class: "st-standouts", "data-testid": "stats-superlatives",
            for (i, row) in rows.iter().enumerate() {
                div {
                    key: "{row.label}",
                    // The leading standout carries the accent tint: the grid
                    // is ranked, and an unranked wall of six identical cards
                    // gives a reader nowhere to start.
                    class: if i == 0 { "st-standout lead" } else { "st-standout" },
                    "data-testid": "stats-superlative",
                    div { class: "st-standout-label", "{row.label}" }
                    div { class: "st-standout-headline", "{row.headline}" }
                    div { class: "st-standout-detail", "{row.detail}" }
                }
            }
            if show_note {
                p { class: "st-standouts-note", "data-testid": "stats-superlatives-note",
                    {fastest_read_note()}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
