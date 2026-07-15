//! Estimated-pages aggregation for the stats page's Pages tile (#1029).
//!
//! **Sourcing model.** No page or word data exists anywhere in the pipeline:
//! `reading_sessions` carries only `seconds_read`, and EPUB indexing records
//! no text-length metric. Of the three options the issue considered — a
//! per-book estimate persisted at index time, a per-session estimate from
//! CFI deltas, or an accumulated words-read figure reported by the reader —
//! the persisted-at-index-time option would require adding a field to
//! `ebook::IndexedBook`, the struct every sync-writer test in the crate
//! constructs field-by-field (no `..Default::default()`), so it ripples
//! into a dozen unrelated test files. The CFI/reader-accumulator options
//! need new client instrumentation (`SessionReport` has no position delta).
//!
//! Instead this computes the estimate **on demand, at stats-query time**:
//! for each book finished in the window (`journal_entries.progress = 100`,
//! same source [`super::finished_books`] uses), resolve its EPUB path via
//! [`crate::book_file_path`] and estimate its word count via
//! [`crate::ebook::estimate_word_count`] (spine text, tags stripped, split
//! on whitespace). Words convert to pages via [`WORDS_PER_PAGE`], a standard
//! prose estimate. The window is normally a handful of books, and the
//! result rides the same [`super::STATS_TTL_SECS`] cache as the rest of
//! [`super::StatsSummary`], so this doesn't add a query-time cost path
//! distinct from what the tile already pays. A book with no EPUB file, an
//! unreadable one, or an empty spine is skipped rather than failing the
//! whole tile — the number stays honest (an underestimate, never a
//! fabricated one) and degrades to `None` (the tile's em-dash state) only
//! when *no* finished book in the window yielded an estimate.

use std::path::PathBuf;

use sqlx::{Row, SqlitePool};

use super::StatsError;

/// Words per printed page, the standard prose estimate (the same ballpark
/// self-publishing/KDP page-count calculators use for a 6x9 trade
/// paperback). Not exact — spine text length is itself an estimate — but
/// documented and consistent, which is what AC1's "clearly-labelled
/// estimated-page count" asks for.
const WORDS_PER_PAGE: f64 = 275.0;

/// Estimated pages read in the window: the word-count sum over every book
/// finished within it (see the module doc for the full model), divided by
/// [`WORDS_PER_PAGE`]. `None` when nothing in the window yielded an
/// estimate — no finished books, none with a resolvable EPUB, or every
/// resolvable one failed to parse.
pub(super) async fn pages_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<i64>, StatsError> {
    let book_ids = finished_book_ids(pool, user_id, start).await?;
    let mut paths = Vec::with_capacity(book_ids.len());
    for id in book_ids {
        if let Some(path) = crate::book_file_path(pool, id, "EPUB").await? {
            paths.push(path);
        }
    }
    if paths.is_empty() {
        return Ok(None);
    }
    // EPUB parsing is blocking zip/XML work — run it off the async runtime
    // rather than stalling other in-flight requests, same convention as
    // `sync::books::reconcile_covers` and friends.
    let total_words = tokio::task::spawn_blocking(move || sum_word_counts(&paths))
        .await
        .ok()
        .flatten();
    Ok(total_words.map(words_to_pages))
}

/// Convert a word total to an estimated page count via [`WORDS_PER_PAGE`],
/// rounding to the nearest page rather than truncating (a 260-word "book"
/// is one page, not zero).
fn words_to_pages(words: i64) -> i64 {
    (words as f64 / WORDS_PER_PAGE).round() as i64
}

/// Distinct `books.id` for the user's hundred-percent journal entries in the
/// window — the same finished-book set [`super::finished_books`] surfaces,
/// but ids (for [`crate::book_file_path`]) rather than display rows.
async fn finished_book_ids(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<i64>, StatsError> {
    let rows = sqlx::query(
        "SELECT DISTINCT b.id AS id
         FROM journal_entries j
         JOIN books b ON b.uuid = j.book_uuid
         WHERE j.user_id = ? AND j.progress = 100 AND j.created_at >= ?",
    )
    .bind(user_id)
    .bind(start)
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

/// Sum word-count estimates across `paths`, skipping any file that fails to
/// open or parse. `None` when every path failed — distinguishes "zero
/// finished books had readable text" from "every finished book is a
/// zero-word epub" (the latter never happens in practice, but the `Option`
/// keeps the em-dash state honest rather than overloading `0`).
fn sum_word_counts(paths: &[PathBuf]) -> Option<i64> {
    let mut total = 0i64;
    let mut any = false;
    for path in paths {
        let Ok(mut doc) = epub::doc::EpubDoc::new(path) else {
            continue;
        };
        if let Some(words) = crate::ebook::estimate_word_count(&mut doc) {
            total += words;
            any = true;
        }
    }
    any.then_some(total)
}

#[cfg(test)]
mod tests;
