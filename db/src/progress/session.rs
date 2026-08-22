//! Session-report recording: batched inserts into the per-format
//! `reading_sessions` / `listening_sessions` tables, replay-idempotent via
//! the `(user_id, client_id)` partial unique index (migration 0052).

use omnibus_shared::{ProgressFormat, SessionReport};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::resolve_canonical_book_uuid_exec;

use super::ProgressError;

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
    // migration 0052: a report the client replayed because it never saw the
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
    // Same BEGIN IMMEDIATE reasoning as `upsert_progress` above (#1862).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let result = record_session_tx(&mut tx, user_id, report).await?;
    tx.commit().await?;
    Ok(result)
}
