//! Estimated-pages aggregation for the stats page's Pages tile. The per-book
//! word count is persisted on `books.word_count` at index time (see
//! [`crate::ebook::estimate_word_count`] and migration `0049`), so this is a
//! single `SUM` over the books finished in the window — no EPUB is opened at
//! query time. The word total converts to pages via [`WORDS_PER_PAGE`].
//! Degrades to `None` (the tile's em-dash state) when no finished book in the
//! window has a stored word count.

use sqlx::SqlitePool;

use super::{StatsError, FINISHED_EVENTS};

/// Words per printed page, the standard prose estimate (the same ballpark
/// self-publishing/KDP page-count calculators use for a 6x9 trade
/// paperback). Not exact — the stored word count is itself a spine-text
/// estimate — but documented and consistent, which is what the tile's
/// "clearly-labelled estimated-page count" asks for.
const WORDS_PER_PAGE: f64 = 275.0;

/// Estimated pages read in the window: the persisted word-count sum over
/// every distinct book finished within it (same completion definition as
/// [`super::finished_books`] — a 100% journal entry or an explicit
/// read-status `finished`), divided by [`WORDS_PER_PAGE`]. A book finished
/// twice counts once. `None` when no finished book in the window has a stored
/// word count — none finished, or every finished one is audio-only /
/// not-yet-backfilled (NULL `word_count`).
pub(super) async fn pages_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<i64>, StatsError> {
    // DISTINCT-book subquery first so a book finished both ways (or with
    // multiple journal entries) contributes its word count once; `SUM` over
    // zero rows is SQL NULL, which maps straight to the em-dash `None`.
    let sql = format!(
        "SELECT SUM(word_count) FROM (
             SELECT DISTINCT b.uuid AS uuid, b.word_count AS word_count
             FROM ({FINISHED_EVENTS}) f
             JOIN books b ON b.uuid = f.book_uuid
             WHERE f.finished_at >= ? AND b.word_count IS NOT NULL
         )"
    );
    let total_words: Option<i64> = sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?;
    Ok(total_words.map(words_to_pages))
}

/// Convert a word total to an estimated page count via [`WORDS_PER_PAGE`],
/// rounding to the nearest page rather than truncating (a 260-word "book"
/// is one page, not zero).
fn words_to_pages(words: i64) -> i64 {
    // Word totals sit far below f64's 2^52 exact-integer range, and dividing
    // by `WORDS_PER_PAGE` only shrinks the value, so both casts are in-range.
    #[allow(clippy::cast_precision_loss, clippy::cast_possible_truncation)]
    let pages = (words as f64 / WORDS_PER_PAGE).round() as i64;
    pages
}

#[cfg(test)]
mod tests;
