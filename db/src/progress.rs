//! F2.1 progress sync — server-authoritative reading/listening position and
//! batched session reports.
//!
//! Position upserts are last-write-wins on `(user_id, book_id, format)`; the
//! upsert always bumps `updated_at` to now so a newly-opened client can sync
//! forward by reading the row back. Session inserts write into the
//! per-format `reading_sessions` / `listening_sessions` tables.

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate, SessionReport};
use sqlx::{Row, SqlitePool};

use crate::resolve_book_id_by_uuid;

#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
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
/// server-authoritative record. Resolves `book_uuid` → `book_id`; returns
/// `ProgressError::BookNotFound` when the uuid is unknown.
pub async fn upsert_progress(
    pool: &SqlitePool,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    let book_id = resolve_book_id_by_uuid(pool, &update.book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let fmt = format_str(update.format);
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_id, format, epub_cfi, audio_position_seconds, updated_at)
         VALUES (?, ?, ?, ?, ?, strftime('%s','now'))
         ON CONFLICT(user_id, book_id, format) DO UPDATE SET
             epub_cfi = excluded.epub_cfi,
             audio_position_seconds = excluded.audio_position_seconds,
             updated_at = strftime('%s','now')",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .bind(update.epub_cfi.as_deref())
    .bind(update.audio_position_seconds)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_id = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .fetch_one(pool)
    .await?;
    Ok(ProgressRecord {
        book_uuid: update.book_uuid.clone(),
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
    let Some(book_id) = resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let fmt = format_str(format);
    let Some(row) = sqlx::query(
        "SELECT format, epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_id = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ProgressRecord {
        book_uuid: book_uuid.to_string(),
        format: parse_format(row.try_get::<String, _>("format")?.as_str()),
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
    }))
}

/// Append one session row to the per-format table. Returns `Ok(true)` when
/// a row was inserted and `Ok(false)` when the report was skipped because
/// the `book_uuid` is unknown (a session that outlived the book it referred
/// to is best-effort telemetry, not an integrity failure). The handler
/// surfaces the inserted count to the client so it can tell which queued
/// reports actually persisted.
pub async fn record_session(
    pool: &SqlitePool,
    user_id: i64,
    report: &SessionReport,
) -> Result<bool, ProgressError> {
    let Some(book_id) = resolve_book_id_by_uuid(pool, &report.book_uuid).await? else {
        return Ok(false);
    };
    match report.format {
        ProgressFormat::Epub => {
            sqlx::query(
                "INSERT INTO reading_sessions
                    (user_id, book_id, started_at, ended_at, seconds_read, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(book_id)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(pool)
            .await?;
        }
        ProgressFormat::Audio => {
            sqlx::query(
                "INSERT INTO listening_sessions
                    (user_id, book_id, started_at, ended_at, seconds_listened, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(book_id)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests;
