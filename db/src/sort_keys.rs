//! Denormalized sort-key columns carried on `books` for the keyset
//! landing path. Today that is `series_sort` (the primary series name),
//! computed at index time by the sync writers via [`series_sort_value`] and
//! backfilled once at boot by [`backfill_series_sort`] for rows indexed
//! before migration 0028. Mirrors the `author_sort` precedent — a sort key
//! denormalized onto the row so a `(library_id, series_sort, series_index,
//! id)` keyset seek replaces a join-and-temp-sort.
//!
//! Like `normalize`, this denormalizes **scanned** metadata only;
//! `metadata_overrides` is a read layer and is deliberately ignored, so a
//! book's sort position tracks its scanned series, not an override.

use omnibus_shared::EbookMetadata;
use sqlx::SqlitePool;

/// Errors returned by [`backfill_series_sort`].
#[derive(Debug, thiserror::Error)]
pub enum SortKeysError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The `series_sort` value to store for a book: its primary series name
/// (trimmed, with any embedded `#N` / ", Book N" index stripped — #1912),
/// or `None` when the book has no (non-empty) series. Delegates to
/// [`crate::helpers::cleaned_series_name`] so this column and the
/// `books_series_link` write it sits alongside always agree on the same
/// cleaned name.
pub fn series_sort_value(m: &EbookMetadata) -> Option<String> {
    crate::helpers::cleaned_series_name(m)
}

/// One-time backfill of `books.series_sort` for rows indexed before migration
/// 0028. Idempotent and cheap once caught up, so it runs on every boot from
/// `init_db`: it only selects rows whose `series_sort` is still NULL **and**
/// that actually have a series link, so a seriesless book (correctly left
/// NULL) is never re-touched and the query is a no-op once every linked book
/// is filled. The value mirrors the read-path "primary series" pick
/// (alphabetically first by name), matching `BOOK_COLUMNS`' `series_json`.
pub async fn backfill_series_sort(pool: &SqlitePool) -> Result<(), SortKeysError> {
    // TRIM mirrors the write-time `series_sort_value` so backfilled rows match
    // freshly-indexed ones. The guard requires a non-whitespace linked series
    // name, so a (degenerate) whitespace-only series never traps a row in a
    // re-touch loop — it stays NULL and is skipped next boot.
    sqlx::query(
        r"
        UPDATE books
           SET series_sort = (
               SELECT TRIM(s.name)
                 FROM books_series_link bsl
                 JOIN series s ON s.id = bsl.series
                WHERE bsl.book = books.id
                ORDER BY s.name
                LIMIT 1
           )
         WHERE series_sort IS NULL
           AND EXISTS (
               SELECT 1
                 FROM books_series_link bsl
                 JOIN series s ON s.id = bsl.series
                WHERE bsl.book = books.id
                  AND TRIM(s.name) <> ''
           )
        ",
    )
    .execute(pool)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
