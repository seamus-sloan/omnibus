//! Server-authoritative reading/listening position sync plus batched
//! session reports. Position upserts are last-write-wins on
//! `(user_id, book_uuid, format)`; session inserts go to the per-format
//! `reading_sessions` / `listening_sessions` tables. All rows soft-reference
//! the durable `books.uuid` (F1 — no FK, no cascade), resolved through the
//! same merged-uuid-aware canonical resolver so a format-merged uuid stores
//! the surviving book's identity.

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate, SessionReport};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{resolve_canonical_book_uuid, resolve_canonical_book_uuid_exec};

#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for ProgressError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => ProgressError::Sqlx(inner),
            // `resolve_book_id_by_uuid` is the only `BooksError`-returning
            // call this module makes, and it never decodes JSON, so the
            // `OverridesJson` variant is unreachable here in practice. Fold
            // it into a generic decode error rather than panicking so a
            // future caller can't introduce an unhandled path silently.
            crate::books::BooksError::OverridesJson(inner) => {
                ProgressError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

fn format_str(f: ProgressFormat) -> &'static str {
    match f {
        ProgressFormat::Epub => "epub",
        ProgressFormat::Audio => "audio",
    }
}

fn parse_format(raw: &str) -> ProgressFormat {
    // The CHECK constraint on `reading_progress.format` rules out anything
    // other than these two values, so an unknown string here would be a
    // schema-level invariant breach. Default to Epub rather than panic.
    match raw {
        "audio" => ProgressFormat::Audio,
        _ => ProgressFormat::Epub,
    }
}

/// Upsert a position row for `(user, book, format)` and return the new
/// server-authoritative record. Resolves the request uuid to the **canonical**
/// `books.uuid` (keeping the `BookNotFound` guard — you cannot record progress
/// for a book the server has never indexed) and stores/keys on it.
pub async fn upsert_progress(
    pool: &SqlitePool,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &update.book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let fmt = format_str(update.format);
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, epub_cfi, audio_position_seconds, updated_at)
         VALUES (?, ?, ?, ?, ?, strftime('%s','now'))
         ON CONFLICT(user_id, book_uuid, format) DO UPDATE SET
             epub_cfi = excluded.epub_cfi,
             audio_position_seconds = excluded.audio_position_seconds,
             updated_at = strftime('%s','now')",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(fmt)
    .bind(update.epub_cfi.as_deref())
    .bind(update.audio_position_seconds)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_uuid = ? AND format = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(fmt)
    .fetch_one(pool)
    .await?;
    Ok(ProgressRecord {
        book_uuid,
        format: update.format,
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
    })
}

/// Fetch the current position row for `(user, book_uuid, format)`. Returns
/// `Ok(None)` for an unknown book uuid or for a book with no row for that
/// format yet.
pub async fn get_progress(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    format: ProgressFormat,
) -> Result<Option<ProgressRecord>, ProgressError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let fmt = format_str(format);
    let Some(row) = sqlx::query(
        "SELECT format, epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_uuid = ? AND format = ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(fmt)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ProgressRecord {
        book_uuid: canonical,
        format: parse_format(row.try_get::<String, _>("format")?.as_str()),
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
    }))
}

/// Append one session row inside an existing transaction. Returns `Ok(true)`
/// when a row was inserted and `Ok(false)` when the report was skipped
/// because the `book_uuid` resolves to neither a `books` row nor a
/// `merged_uuids` entry (best-effort telemetry — a session that outlived its
/// book is not an integrity failure). A format-merged or auto-attached uuid
/// resolves to the surviving book and is recorded.
///
/// The caller is responsible for committing or rolling back the transaction.
/// Use this variant when inserting a batch so the entire batch is atomic.
pub async fn record_session_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    report: &SessionReport,
) -> Result<bool, ProgressError> {
    // Resolve through the same merged-uuid-aware path as `upsert_progress`,
    // so a uuid that was format-merged or auto-attached after the session
    // started still records against the surviving book instead of being
    // silently dropped. `Ok(false)` now means "unknown in neither `books`
    // nor `merged_uuids`".
    let Some(book_uuid) = resolve_canonical_book_uuid_exec(&mut **tx, &report.book_uuid).await?
    else {
        return Ok(false);
    };
    match report.format {
        ProgressFormat::Epub => {
            sqlx::query(
                "INSERT INTO reading_sessions
                    (user_id, book_uuid, started_at, ended_at, seconds_read, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(&book_uuid)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(&mut **tx)
            .await?;
        }
        ProgressFormat::Audio => {
            sqlx::query(
                "INSERT INTO listening_sessions
                    (user_id, book_uuid, started_at, ended_at, seconds_listened, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(&book_uuid)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(true)
}

/// Append one session row to the per-format table. Returns `Ok(true)` when
/// a row was inserted and `Ok(false)` when the report was skipped because
/// the `book_uuid` is unknown. The handler surfaces the inserted count to
/// the client so it can tell which queued reports actually persisted.
///
/// For batch inserts, prefer [`record_session_tx`] inside a caller-managed
/// transaction so the entire batch rolls back atomically on error.
pub async fn record_session(
    pool: &SqlitePool,
    user_id: i64,
    report: &SessionReport,
) -> Result<bool, ProgressError> {
    let mut tx = pool.begin().await?;
    let result = record_session_tx(&mut tx, user_id, report).await?;
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod tests;
