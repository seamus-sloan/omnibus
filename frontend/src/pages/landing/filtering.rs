//! Client-side filter helpers for the landing page's search path.
//!
//! Applies the user's [`ViewFilters`] over the (capped) search result set.
//! Browse is filtered server-side and the facet sidebar was retired in favor of shelves,
//! so only the filter predicate + table format-badge label remain here.
// `format_badge_label` feeds the web table only — dead on the mobile build.
#![cfg_attr(feature = "mobile", allow(dead_code))]

use omnibus_shared::{EbookMetadata, ViewFilters};

fn matches_filters(book: &EbookMetadata, filters: &ViewFilters) -> bool {
    // Allocation-free membership checks: filter buckets are typically tiny
    // (a handful of selected chips), so a nested `any().any()` is faster
    // than building a fresh HashSet per book on every filter pass.
    if !filters.authors.is_empty()
        && !filters
            .authors
            .iter()
            .any(|a| book.creators.iter().any(|c| &c.name == a))
    {
        return false;
    }
    if !filters.series.is_empty() {
        let series = book.series.as_deref().unwrap_or("");
        if !filters.series.iter().any(|s| s == series) {
            return false;
        }
    }
    if !filters.formats.is_empty()
        && !filters
            .formats
            .iter()
            .any(|f| book.formats.iter().any(|bf| bf.eq_ignore_ascii_case(f)))
    {
        return false;
    }
    if !filters.tags.is_empty()
        && !filters
            .tags
            .iter()
            .any(|t| book.subjects.iter().any(|s| s == t))
    {
        return false;
    }
    if !filters.genres.is_empty()
        && !filters
            .genres
            .iter()
            .any(|g| book.genres.iter().any(|bg| bg == g))
    {
        return false;
    }
    true
}

/// Keep only the books matching every active sidebar facet. An empty filter
/// set clones the input through unchanged.
pub(crate) fn apply_filters(books: &[EbookMetadata], filters: &ViewFilters) -> Vec<EbookMetadata> {
    if filters.is_empty() {
        return books.to_vec();
    }
    books
        .iter()
        .filter(|b| matches_filters(b, filters))
        .cloned()
        .collect()
}

/// Short badge text for the table's Formats column. Stays compact so a row
/// with two formats doesn't overflow the cell.
pub(crate) fn format_badge_label(raw: &str) -> String {
    raw.trim().to_ascii_uppercase()
}

#[cfg(test)]
mod tests;
