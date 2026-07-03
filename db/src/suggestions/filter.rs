//! Pure candidate-filtering for F3.3 suggestions. Given the source book's
//! authors/series and Hardcover co-listed candidates (already ranked by
//! co-listing count, descending), trim to the top read-alikes that are genuine
//! *new* discoveries: different author, different series, and an entry point
//! (series-starter or standalone). No I/O; called by [`crate::suggestions::cascade`].

use std::collections::HashSet;

/// A Hardcover co-listed book competing for a slot in the strip. `author` is
/// the primary contributor; `series` / `series_position` come from Hardcover's
/// `book_series` edge (both may be absent for a standalone).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub hardcover_id: i64,
    pub slug: Option<String>,
    pub title: String,
    pub author: String,
    pub series: Option<String>,
    pub series_position: Option<f64>,
    pub list_count: i64,
    pub cover_url: Option<String>,
}

/// Trim + lowercase for case/whitespace-insensitive name comparison.
fn norm(s: &str) -> String {
    s.trim().to_lowercase()
}

/// An "entry point" is a standalone (no series) or a series-starter
/// (`position <= 1`). A book in a series with **unknown** position is treated
/// as *not* an entry point: we can't prove it's the first, and the roadmap
/// prioritizes never dropping a reader mid-series over surfacing a maybe-starter.
pub fn is_entry_point(series: Option<&str>, position: Option<f64>) -> bool {
    match series {
        None => true,
        Some(name) if name.trim().is_empty() => true,
        Some(_) => position.is_some_and(|p| p <= 1.0),
    }
}

/// True when the candidate shares an author with the source book
/// (case-insensitive). Such candidates are excluded — the strip is for
/// *other* authors.
pub fn is_same_author(candidate_author: &str, source_authors: &[String]) -> bool {
    let c = norm(candidate_author);
    source_authors.iter().any(|a| norm(a) == c)
}

/// True when the candidate is in the same series as the source book
/// (case-insensitive, both must name a series).
pub fn is_same_series(candidate_series: Option<&str>, source_series: Option<&str>) -> bool {
    match (candidate_series, source_series) {
        (Some(c), Some(s)) => !c.trim().is_empty() && norm(c) == norm(s),
        _ => false,
    }
}

/// Apply the full filter and return at most `limit` survivors, preserving the
/// input order (callers pass candidates pre-sorted by co-listing count desc).
/// Duplicate `hardcover_id`s are collapsed to their first occurrence.
pub fn filter_candidates(
    candidates: &[Candidate],
    source_authors: &[String],
    source_series: Option<&str>,
    limit: usize,
) -> Vec<Candidate> {
    let mut seen: HashSet<i64> = HashSet::new();
    let mut out: Vec<Candidate> = Vec::new();
    for c in candidates {
        if out.len() >= limit {
            break;
        }
        if !seen.insert(c.hardcover_id) {
            continue;
        }
        if is_same_author(&c.author, source_authors) {
            continue;
        }
        if is_same_series(c.series.as_deref(), source_series) {
            continue;
        }
        if !is_entry_point(c.series.as_deref(), c.series_position) {
            continue;
        }
        out.push(c.clone());
    }
    out
}
