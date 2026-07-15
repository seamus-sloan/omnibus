//! Estimated-pages aggregation for the stats page's Pages tile. No page or
//! word data is persisted anywhere, so this resolves each window-finished
//! book's EPUB path and estimates its word count on demand at stats-query
//! time, converting to pages via [`WORDS_PER_PAGE`]. Degrades to `None`
//! (the tile's em-dash state) only when nothing in the window yields an estimate.

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
    // `sync::books::reconcile_covers` and friends. `?` propagates a panic or
    // cancellation as `StatsError::PagesTask` rather than masking it as "no
    // data" — a spurious em-dash would hide a real bug.
    let total_words = tokio::task::spawn_blocking(move || sum_word_counts(&paths)).await?;
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
