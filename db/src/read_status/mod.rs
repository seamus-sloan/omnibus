//! Per-user read/unread state CRUD. One row per `(user, book)`, last-write-wins
//! on the status, soft-referencing the durable `books.uuid` (no FK/cascade)
//! through the merged-uuid-aware canonical resolver — mirroring `ratings`. A
//! pruned book detaches its read state rather than deleting it; the
//! missing-files GC keeps any book a read-status row references.
//!
//! `finished_at` is stamped the moment a book becomes `finished` and preserved
//! while it stays finished, so the reading-stats "finished" aggregations can
//! window completions the same way the journal path windows `created_at`.
//! Setting any non-finished status clears it back to NULL.

use omnibus_shared::{ReadStatus, ReadStatusRecord, SetReadStatus};
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

#[derive(Debug, thiserror::Error)]
pub enum ReadStatusError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for ReadStatusError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            // `resolve_canonical_book_uuid` is the only `BooksError`-returning
            // call this module makes and it never decodes JSON, so the
            // `OverridesJson` variant is unreachable here. Fold it into a decode
            // error rather than panicking, mirroring `ratings`/`progress`.
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// Row → record, reading the persisted status token fail-open.
fn record_from_row(
    book_uuid: String,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ReadStatusRecord, sqlx::Error> {
    Ok(ReadStatusRecord {
        book_uuid,
        status: ReadStatus::from_db(row.try_get::<String, _>("status")?.as_str()),
        updated_at: row.try_get::<i64, _>("updated_at")?,
        finished_at: row.try_get::<Option<i64>, _>("finished_at")?,
    })
}

/// Upsert the read state for `(user, book)` and return the new
/// server-authoritative record. Resolves the request uuid to the **canonical**
/// `books.uuid` (keeping the `BookNotFound` guard — you cannot record a read
/// state for a book the server has never indexed) and stores/keys on it.
///
/// `finished_at` is set to now when transitioning into `finished`, preserved
/// when re-affirming `finished`, and cleared to NULL for `unread` / `reading`.
pub async fn set_read_status(
    pool: &SqlitePool,
    user_id: i64,
    update: &SetReadStatus,
) -> Result<ReadStatusRecord, ReadStatusError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &update.book_uuid)
        .await?
        .ok_or(ReadStatusError::BookNotFound)?;
    let status = update.status.as_str();
    // `finished_at` binds `status` a second time so the CASE picks now-or-null
    // on insert; the ON CONFLICT arm COALESCEs against the existing value so a
    // re-affirmed `finished` keeps its original completion moment.
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, ?, strftime('%s','now'),
                 CASE WHEN ? = 'finished' THEN strftime('%s','now') ELSE NULL END)
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET
             status = excluded.status,
             updated_at = strftime('%s','now'),
             finished_at = CASE
                 WHEN excluded.status = 'finished'
                     THEN COALESCE(book_read_status.finished_at, strftime('%s','now'))
                 ELSE NULL
             END",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(status)
    .bind(status)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT status, updated_at, finished_at FROM book_read_status
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .fetch_one(pool)
    .await?;
    Ok(record_from_row(book_uuid, &row)?)
}

/// Fetch the current read state for `(user, book_uuid)`. Returns `Ok(None)` for
/// an unknown book uuid or for a book the user has no row for yet — callers
/// treat the absence as [`ReadStatus::Unread`].
pub async fn get_read_status(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<ReadStatusRecord>, ReadStatusError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let Some(row) = sqlx::query(
        "SELECT status, updated_at, finished_at FROM book_read_status
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(record_from_row(canonical, &row)?))
}

#[cfg(test)]
mod tests;
