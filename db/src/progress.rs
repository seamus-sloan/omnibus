//! Server-authoritative reading/listening position sync plus batched
//! session reports. Position upserts keep the latest *reader* clock on
//! `(user_id, book_uuid, format)`; session inserts go to the per-format
//! `reading_sessions` / `listening_sessions` tables. All rows soft-reference
//! the durable `books.uuid` (no FK, no cascade), resolved through the
//! same merged-uuid-aware canonical resolver so a format-merged uuid stores
//! the surviving book's identity.

use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, ChapterInfo, ProgressFormat,
    ProgressRecord, ProgressUpdate, ResumePoint, SessionReport,
};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{hls, resolve_canonical_book_uuid, resolve_canonical_book_uuid_exec};

#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<hls::HlsError> for ProgressError {
    fn from(e: hls::HlsError) -> Self {
        match e {
            hls::HlsError::Db(inner) => ProgressError::Sqlx(inner),
        }
    }
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

/// Upsert a position row for `(user, book, format)` and return the
/// server-authoritative record — which is the **winning** row, not necessarily
/// the one just submitted. Resolves the request uuid to the **canonical**
/// `books.uuid` (keeping the `BookNotFound` guard — you cannot record progress
/// for a book the server has never indexed) and stores/keys on it.
///
/// Ordered on `ProgressUpdate::client_updated_at` — the reader's clock — rather
/// than on arrival. A position queued offline can reach the server hours after
/// it was read, long after another device has moved further along, and arrival
/// order would let the stale one win and then relabel itself as newest. An
/// update with no client clock is stamped with the server's, which reduces to
/// the old last-write-wins for callers that don't send one.
///
/// Returning the winner rather than 409ing is deliberate: the caller's next act
/// is to cache what it got back, and handing it the position that actually
/// stands is what converges the two devices.
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
            (user_id, book_uuid, format, epub_cfi, audio_position_seconds,
             updated_at, client_updated_at)
         VALUES (?, ?, ?, ?, ?, strftime('%s','now'),
                 COALESCE(?, strftime('%s','now')))
         ON CONFLICT(user_id, book_uuid, format) DO UPDATE SET
             epub_cfi = excluded.epub_cfi,
             audio_position_seconds = excluded.audio_position_seconds,
             updated_at = strftime('%s','now'),
             client_updated_at = excluded.client_updated_at
         WHERE excluded.client_updated_at >=
               COALESCE(reading_progress.client_updated_at, 0)",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(fmt)
    .bind(update.epub_cfi.as_deref())
    .bind(update.audio_position_seconds)
    .bind(update.client_updated_at)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT epub_cfi, audio_position_seconds, updated_at, client_updated_at
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
        client_updated_at: row.try_get::<Option<i64>, _>("client_updated_at")?,
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
        "SELECT format, epub_cfi, audio_position_seconds, updated_at, client_updated_at
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
        client_updated_at: row.try_get::<Option<i64>, _>("client_updated_at")?,
    }))
}

/// Upsert the playback rate for `(user, book)` and return the saved preference.
pub async fn set_playback_rate(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    update: &AudiobookPlaybackRateUpdate,
) -> Result<AudiobookPlaybackRateRecord, ProgressError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let row = sqlx::query(
        "INSERT INTO audiobook_playback_preferences
            (user_id, book_uuid, playback_rate, updated_at)
         VALUES (?, ?, ?, strftime('%s', 'now'))
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET
            playback_rate = excluded.playback_rate,
            updated_at = strftime('%s', 'now')
         RETURNING playback_rate, updated_at",
    )
    .bind(user_id)
    .bind(&canonical)
    .bind(update.playback_rate)
    .fetch_one(pool)
    .await?;
    Ok(AudiobookPlaybackRateRecord {
        book_uuid: canonical,
        playback_rate: row.try_get("playback_rate")?,
        updated_at: row.try_get("updated_at")?,
    })
}

/// Fetch the user's playback-rate preference for a book, if one exists.
pub async fn get_playback_rate(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<AudiobookPlaybackRateRecord>, ProgressError> {
    let canonical = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let Some(row) = sqlx::query(
        "SELECT playback_rate, updated_at
         FROM audiobook_playback_preferences
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&canonical)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(AudiobookPlaybackRateRecord {
        book_uuid: canonical,
        playback_rate: row.try_get("playback_rate")?,
        updated_at: row.try_get("updated_at")?,
    }))
}

/// The user's most recent progress rows across both formats, newest first.
/// Feeds the "pick up where you left off" surfaces via [`resume_points`].
pub async fn recent_progress(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> Result<Vec<ProgressRecord>, ProgressError> {
    let rows = sqlx::query(
        "SELECT book_uuid, format, epub_cfi, audio_position_seconds, updated_at,
                client_updated_at
         FROM reading_progress
         WHERE user_id = ?
         ORDER BY updated_at DESC, book_uuid
         LIMIT ?",
    )
    .bind(user_id)
    .bind(limit)
    .fetch_all(pool)
    .await?;
    rows.into_iter()
        .map(|row| {
            Ok(ProgressRecord {
                book_uuid: row.try_get::<String, _>("book_uuid")?,
                format: parse_format(row.try_get::<String, _>("format")?.as_str()),
                epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
                audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
                updated_at: row.try_get::<i64, _>("updated_at")?,
                client_updated_at: row.try_get::<Option<i64>, _>("client_updated_at")?,
            })
        })
        .collect()
}

/// [`recent_progress`] joined with book metadata and, for audio rows, the
/// whole-book duration + chapter position. Rows whose book has since been
/// deleted (progress soft-references `books.uuid` — no cascade) are skipped
/// rather than erroring, so one ghosted book can't blank the landing card.
pub async fn resume_points(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> Result<Vec<ResumePoint>, ProgressError> {
    let records = recent_progress(pool, user_id, limit).await?;
    let mut points = Vec::with_capacity(records.len());
    for record in records {
        let Some(book) = crate::get_book_by_uuid(pool, &record.book_uuid).await? else {
            continue;
        };
        let audio = match record.format {
            ProgressFormat::Audio => audio_totals(pool, &record.book_uuid, &record).await?,
            ProgressFormat::Epub => None,
        };
        let (total_duration_seconds, chapter_number, chapter_count) = match audio {
            Some((dur, ch_no, ch_count)) => (Some(dur), ch_no, ch_count),
            None => (None, None, None),
        };
        points.push(ResumePoint {
            record,
            book,
            total_duration_seconds,
            chapter_number,
            chapter_count,
        });
    }
    Ok(points)
}

/// Whole-book duration and chapter position for an audio progress row.
/// `None` when the book has no resolvable audio file (e.g. the file was
/// removed after the position was saved).
async fn audio_totals(
    pool: &SqlitePool,
    uuid: &str,
    record: &ProgressRecord,
) -> Result<Option<(f64, Option<i64>, Option<i64>)>, ProgressError> {
    let Some(resolved) = hls::resolve_audiobook(pool, uuid).await? else {
        return Ok(None);
    };
    let parts = hls::get_parts(pool, resolved.book_file_id).await?;
    let total: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    let mut chapters = hls::get_chapters(pool, resolved.book_file_id).await?;
    chapters.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    let position = record.audio_position_seconds.unwrap_or(0.0);
    let number = chapter_number_at(&chapters, position);
    let count = (!chapters.is_empty()).then_some(chapters.len() as i64);
    Ok(Some((total, number, count)))
}

/// 1-based chapter number at `elapsed` seconds, mirroring the player's
/// index-plus-one display (not the stored `file_chapters.ordinal`, which is
/// container-supplied and not guaranteed dense).
fn chapter_number_at(chapters: &[ChapterInfo], elapsed: f64) -> Option<i64> {
    if chapters.is_empty() {
        return None;
    }
    let idx = chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1);
    Some(idx as i64 + 1)
}

/// Append one session row inside an existing transaction. Returns `Ok(true)`
/// when a row was inserted and `Ok(false)` when the report was skipped
/// because the `book_uuid` resolves to neither a `books` row nor a
/// `merged_uuids` entry (best-effort telemetry — a session that outlived its
/// book is not an integrity failure). A format-merged or auto-attached uuid
/// resolves to the surviving book and is recorded.
///
/// The caller is responsible for committing or rolling back the transaction.
/// Batch writers that already pre-resolved every uuid via
/// [`crate::resolve_canonical_book_uuids_bulk_exec`] should skip this wrapper
/// and call [`insert_session_tx`] directly to avoid the per-row SELECT.
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
    insert_session_tx(tx, user_id, report, &book_uuid).await?;
    Ok(true)
}

/// Insert one session row into the correct per-format table using a
/// **pre-resolved** canonical `books.uuid`. This is the INSERT-only half of
/// [`record_session_tx`], exposed so batch writers (see `post_sessions`) can
/// pre-resolve every uuid in the batch through
/// [`crate::resolve_canonical_book_uuids_bulk_exec`] and then loop through
/// pure inserts — collapsing an N-report batch's 2N queries into
/// `chunks + N`. The caller is responsible for committing or rolling back
/// the transaction; the caller also owns the "skip on unknown uuid" branch
/// (an entry missing from the bulk map).
pub async fn insert_session_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    report: &SessionReport,
    canonical_uuid: &str,
) -> Result<(), ProgressError> {
    // OR IGNORE against the partial-unique `(user_id, client_id)` index from
    // migration 0050: a report the client replayed because it never saw the
    // reply collapses onto the row it already wrote instead of doubling the
    // reading time it represents. Reports without a client id are
    // unconstrained and insert as before.
    match report.format {
        ProgressFormat::Epub => {
            sqlx::query(
                "INSERT OR IGNORE INTO reading_sessions
                    (user_id, book_uuid, started_at, ended_at, seconds_read, device_id, client_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(canonical_uuid)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .bind(report.client_id.as_deref())
            .execute(&mut **tx)
            .await?;
        }
        ProgressFormat::Audio => {
            sqlx::query(
                "INSERT OR IGNORE INTO listening_sessions
                    (user_id, book_uuid, started_at, ended_at, seconds_listened, device_id, client_id)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(canonical_uuid)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .bind(report.client_id.as_deref())
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// Append one session row to the per-format table. Returns `Ok(true)` when
/// a row was inserted and `Ok(false)` when the report was skipped because
/// the `book_uuid` is unknown. The handler surfaces the inserted count to
/// the client so it can tell which queued reports actually persisted.
///
/// For batch inserts, prefer the pattern in `post_sessions`: pre-resolve
/// every uuid via [`crate::resolve_canonical_book_uuids_bulk_exec`], then
/// loop [`insert_session_tx`] inside a caller-managed transaction so the
/// entire batch rolls back atomically on error and no per-row SELECT fires.
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
