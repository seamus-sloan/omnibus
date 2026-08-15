//! Write-path validation for [`upsert_progress`]: most-recent-wins conflict
//! resolution by client event time, the audio multi-file guard, and the two
//! WARN-only checks (#1861) that flag a rejected write or a large backward
//! jump without changing what gets stored.

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::resolve_canonical_book_uuid_exec;

use super::{format_str, ProgressError};

/// Backward jump in an accepted audio position write that's worth a WARN
/// (~10 minutes). A normal skip-back or chapter re-listen is well under
/// this; the 2026-08-11 chapter-19-to-13 revert was hours.
const AUDIO_BACKWARD_JUMP_THRESHOLD_SECONDS: f64 = 600.0;

/// A near-zero audio write against a row this far into the book is the
/// teardown signature, not a listen (see the refusal in
/// [`upsert_progress_tx`]).
const AUDIO_TEARDOWN_ZERO_CUTOFF_SECONDS: f64 = 1.0;
const AUDIO_TEARDOWN_ZERO_MIN_STORED_SECONDS: f64 = 600.0;

/// Read the current row back as a [`ProgressRecord`] — shared by the
/// normal post-write read-back and the teardown-signature early return.
async fn read_progress_row(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    book_uuid: String,
    format: ProgressFormat,
    fmt: &str,
) -> Result<ProgressRecord, ProgressError> {
    let row = sqlx::query(
        "SELECT epub_cfi, audio_position_seconds, progress_percent, kobo_location,
                book_file_id, updated_at,
                COALESCE(client_updated_at, updated_at) AS client_updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_uuid = ? AND format = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(fmt)
    .fetch_one(&mut **tx)
    .await?;
    Ok(ProgressRecord {
        book_uuid,
        format,
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
        kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
        book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
        client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
    })
}

/// Upsert a position row for `(user, book, format)` and return the
/// **surviving** server-authoritative record. Resolves the request uuid to
/// the **canonical** `books.uuid` (keeping the `BookNotFound` guard — you
/// cannot record progress for a book the server has never indexed) and
/// stores/keys on it.
///
/// The conflict resolution is conditional on `client_updated_at`, not
/// unconditional last-write-wins on receipt order: the stored
/// `MIN(update.client_updated_at, now)` — clamping a fast client clock so it
/// can't pin itself as permanently newest — only overwrites the row when it
/// is `>=` the value already stored. A write with no `client_updated_at`
/// (older client) is treated as "now", matching plain last-write-wins. A
/// rejected (older) write still re-reads and returns the row that won, so
/// the caller learns it is behind rather than seeing its own rejected
/// payload echoed back.
///
/// An accepted write replaces the **whole position** — `epub_cfi`,
/// `progress_percent`, and `kobo_location` are one atomic snapshot, never
/// field-merged across writers. A row therefore always describes a single
/// position; a field a writer can't supply (a Kobo bookmark with no
/// derivable CFI, a web CFI with no span) stays NULL as "not known", and
/// the Kobo sync-out derives the missing half on demand rather than
/// trusting a stale leftover.
///
/// One backstop narrows that rule: an **audio** write naming no
/// `book_file_id`, landing on a row that names one, for a book carrying
/// more than one audio file, is rejected wholly — same shape as a
/// stale-clock rejection, the re-read returns the surviving row. Audio
/// seconds are only meaningful within one file, so accepting such a write
/// would turn the row into "N seconds within an unknown file" and destroy a
/// position another client (e.g. iOS) recorded correctly (#1888). On a
/// single-file book a fileless write still applies — the server resolves
/// the same default file the manifest serves, so nothing is ambiguous.
pub async fn upsert_progress(
    pool: &SqlitePool,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    // BEGIN IMMEDIATE avoids a stale-snapshot 517 on concurrent writes (#1862).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let record = upsert_progress_tx(&mut tx, user_id, update).await?;
    tx.commit().await?;
    Ok(record)
}

/// Transaction-scoped [`upsert_progress`], for callers that batch it with
/// other writes (e.g. the Kobo `put_state` handler) inside one shared
/// `Transaction` so a mid-batch failure rolls back every entry, not just the
/// one that failed. The caller is responsible for committing or rolling back.
pub async fn upsert_progress_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    let book_uuid = resolve_canonical_book_uuid_exec(&mut **tx, &update.book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let fmt = format_str(update.format);

    // Snapshot the row this write would land on, purely for the two WARNs
    // this function logs (#1861) — neither reads back into the upsert itself.
    let existing = existing_progress_snapshot(tx, user_id, &book_uuid, fmt).await?;
    warn_if_rejected_by_timestamp_guard(&book_uuid, update.client_updated_at, existing.0);

    // Teardown signature, last line of defense (#1954): a dying page's
    // media element flushes its reset clock — audio position ~0 — with a
    // perfectly fresh event time, and the client-side gates cannot cover
    // every event ordering every browser invents. Refuse a near-zero
    // audio write against a row that sits far into the book: the write is
    // dropped, the stored row returned. A genuine restart-from-the-start
    // persists within one heartbeat anyway — its next report is already
    // past the cutoff.
    if update.format == ProgressFormat::Audio {
        if let (Some(new), Some(old)) = (update.audio_position_seconds, existing.1) {
            if new < AUDIO_TEARDOWN_ZERO_CUTOFF_SECONDS
                && old > AUDIO_TEARDOWN_ZERO_MIN_STORED_SECONDS
            {
                tracing::warn!(
                    book_uuid = %book_uuid,
                    old_position_seconds = old,
                    new_position_seconds = new,
                    client_updated_at = update.client_updated_at,
                    "refused near-zero audio write against a far-advanced row (teardown signature)"
                );
                return read_progress_row(tx, user_id, book_uuid, update.format, fmt).await;
            }
        }
    }

    // `strftime('%s','now')` returns TEXT; SQLite's default storage-class
    // sort order ranks every INTEGER below every TEXT value regardless of
    // magnitude, so `MIN(<int param>, <text now>)` would always pick the
    // (unclamped) integer side without an explicit CAST here.
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, epub_cfi, audio_position_seconds,
             progress_percent, kobo_location, book_file_id,
             updated_at, client_updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, strftime('%s','now'),
             MIN(
                 COALESCE(?, CAST(strftime('%s','now') AS INTEGER)),
                 CAST(strftime('%s','now') AS INTEGER)
             ))
         ON CONFLICT(user_id, book_uuid, format) DO UPDATE SET
             epub_cfi = excluded.epub_cfi,
             audio_position_seconds = excluded.audio_position_seconds,
             progress_percent = excluded.progress_percent,
             kobo_location = excluded.kobo_location,
             book_file_id = excluded.book_file_id,
             updated_at = strftime('%s','now'),
             client_updated_at = excluded.client_updated_at
         WHERE excluded.client_updated_at >=
             COALESCE(reading_progress.client_updated_at, reading_progress.updated_at)
           AND NOT (
             excluded.format = 'audio'
             AND excluded.book_file_id IS NULL
             AND reading_progress.book_file_id IS NOT NULL
             AND (SELECT COUNT(*) FROM book_files bf
                    JOIN books b ON b.id = bf.book_id
                   WHERE b.uuid = excluded.book_uuid
                     AND bf.format IN ('M4B', 'M4A', 'MP3')) > 1
           )",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(fmt)
    // Blank-to-NULL at the bind, not just at `validate` — the row CHECK
    // treats any non-NULL as a real position, so a whitespace CFI reaching
    // an internal caller that skipped validation would store as an anchor.
    .bind(update.epub_cfi.as_deref().filter(|s| !s.trim().is_empty()))
    .bind(update.audio_position_seconds)
    .bind(update.progress_percent)
    .bind(
        update
            .kobo_location
            .as_deref()
            .filter(|s| !s.trim().is_empty()),
    )
    .bind(update.book_file_id)
    .bind(update.client_updated_at)
    .execute(&mut **tx)
    .await?;

    let record = read_progress_row(tx, user_id, book_uuid, update.format, fmt).await?;

    // If this write was rejected (by either guard above), the row is
    // unchanged, so `new` equals the pre-write snapshot and this can't
    // fire — only a genuinely *accepted* jump ever crosses the threshold.
    if update.format == ProgressFormat::Audio {
        warn_if_audio_jumped_backward(&record, existing.1);
    }

    Ok(record)
}

/// The `(client_updated_at, audio_position_seconds)` a `(user, book, format)`
/// row held immediately before an upsert — `None`/`None` when no row exists
/// yet. Read-only; used solely to detect the two WARN conditions below.
async fn existing_progress_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    book_uuid: &str,
    fmt: &str,
) -> Result<(Option<i64>, Option<f64>), ProgressError> {
    let Some(row) = sqlx::query(
        "SELECT COALESCE(client_updated_at, updated_at) AS client_updated_at,
                audio_position_seconds
         FROM reading_progress
         WHERE user_id = ? AND book_uuid = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .fetch_optional(&mut **tx)
    .await?
    else {
        return Ok((None, None));
    };
    Ok((
        Some(row.try_get::<i64, _>("client_updated_at")?),
        row.try_get::<Option<f64>, _>("audio_position_seconds")?,
    ))
}

/// AC1 (#1861): a write is about to lose to the timestamp guard when the
/// offered stamp — clamped forward to server-now, same as the SQL — is
/// older than the stamp already on the row. Predicted here in Rust rather
/// than read back from the upsert's own `WHERE`, purely for logging; a
/// prediction that ever disagreed with the SQL would still change nothing,
/// since the guard itself is unmodified.
fn warn_if_rejected_by_timestamp_guard(
    book_uuid: &str,
    client_updated_at: Option<i64>,
    stored_client_updated_at: Option<i64>,
) {
    let Some(stored_ts) = stored_client_updated_at else {
        return;
    };
    let now = crate::auth::now_unix();
    let offered = client_updated_at.unwrap_or(now).min(now);
    if offered < stored_ts {
        tracing::warn!(
            book_uuid,
            stored_client_updated_at = stored_ts,
            offered_client_updated_at = offered,
            "progress write rejected by timestamp guard"
        );
    }
}

/// AC2 (#1861): an *accepted* audio write whose new position lands more
/// than [`AUDIO_BACKWARD_JUMP_THRESHOLD_SECONDS`] behind the old one — a
/// rejected write leaves `record.audio_position_seconds` equal to the old
/// value, so this can only fire for a write that actually landed.
fn warn_if_audio_jumped_backward(
    record: &ProgressRecord,
    stored_audio_position_seconds: Option<f64>,
) {
    let (Some(old), Some(new)) = (stored_audio_position_seconds, record.audio_position_seconds)
    else {
        return;
    };
    if old - new > AUDIO_BACKWARD_JUMP_THRESHOLD_SECONDS {
        tracing::warn!(
            book_uuid = %record.book_uuid,
            old_position_seconds = old,
            new_position_seconds = new,
            client_updated_at = record.client_updated_at,
            "accepted audio write moved position backward past threshold"
        );
    }
}
