//! Forward-progress ledger (migrations `0083`, `0093`): turns the mutable
//! position row into an append-only record of how much of a book was covered,
//! and when. `db::stats::pages` reads it; the position write path is the only
//! writer — every site that sets `reading_progress.progress_percent` must
//! observe here too, or the ground that write covered is lost for good.
//!
//! **This layer decides no calendar.** Gains accrue into quarter-hour slots and
//! the day is resolved on the way out, against whatever offset the reading
//! client is on — see `crate::user_offset`. Writing a day here, as `0083` did,
//! is what made the daily pages goal reset at UTC midnight.

use omnibus_shared::ProgressFormat;
use sqlx::{Sqlite, SqlitePool, Transaction};

use super::{format_str, ProgressError};

#[cfg(test)]
mod tests;

/// `settings` KV key holding the UTC day the ledger began recording. Written
/// once by migration `0083`; read by the stats surfaces so they can state the
/// date before which partial reading cannot be reconstructed.
const PAGES_LEDGER_EPOCH_KEY: &str = "pages_ledger_epoch";

/// Width of a ledger bucket, in seconds — a quarter of an hour.
///
/// The granularity a stored gain can be re-bucketed to a local day at: every
/// modern IANA offset is a whole number of quarter-hours, so shifting a slot by
/// one lands it exactly on a day boundary, which an hourly bucket could not do
/// for UTC+05:30, UTC+05:45 or UTC-03:30. Read back by `crate::stats::pages`,
/// which must resolve days on the same grid this writes them on.
pub const SLOT_SECS: i64 = 900;

/// Record an observed whole-book percent for `(user, book, format)` and accrue
/// whatever forward ground it covers into the quarter-hour it was observed in.
///
/// The gain is measured against the ledger's own mark, never against
/// `reading_progress.progress_percent`: an epub write carrying only a CFI nulls
/// that column and `spawn_epub_percent_derivation` refills it off the request
/// path, so the live row is NULL for part of its life and differencing it would
/// silently lose every gain on the surfaces that write CFIs (iOS).
///
/// Two cases accrue nothing on purpose. A book with no mark yet is *baselined*
/// — a device syncing a book it is already 60% through has not just read 60% of
/// it. A backward move is a re-read or a correction, and negative pages read is
/// not a thing; reading that ground again accrues on the way forward.
///
/// `observed_at` is the surviving record's client event time, so a write
/// replayed from an offline outbox lands in the quarter-hour it happened in
/// rather than the one it drained in.
pub(super) async fn observe_percent_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    book_uuid: &str,
    format: ProgressFormat,
    percent: i64,
    observed_at: i64,
) -> Result<(), ProgressError> {
    let fmt = format_str(format);
    let previous: Option<i64> = sqlx::query_scalar(
        "SELECT percent FROM reading_progress_marks
         WHERE user_id = ? AND book_uuid = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .fetch_optional(&mut **tx)
    .await?;

    // Clamped rather than rejected: the column CHECK would turn a client that
    // slipped a stray value past `ProgressUpdate::validate` into a 500 on the
    // position write, and losing a page of telemetry beats losing the position.
    let percent = percent.clamp(0, 100);

    sqlx::query(
        "INSERT INTO reading_progress_marks (user_id, book_uuid, format, percent, updated_at)
         VALUES (?, ?, ?, ?, ?)
         ON CONFLICT(user_id, book_uuid, format) DO UPDATE SET
             percent = excluded.percent,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .bind(percent)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;

    let Some(previous) = previous else {
        return Ok(());
    };
    let gained = percent - previous;
    if gained <= 0 {
        return Ok(());
    }

    // Floor division, and it has to be: Rust's `/` truncates toward zero, which
    // for a pre-1970 `observed_at` would round the slot *up* and file the gain
    // in the quarter-hour after the one it happened in.
    let slot = observed_at.div_euclid(SLOT_SECS);
    sqlx::query(
        "INSERT INTO reading_progress_slots
             (user_id, book_uuid, format, slot, percent_gained, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id, book_uuid, format, slot) DO UPDATE SET
             percent_gained = percent_gained + excluded.percent_gained,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .bind(slot)
    .bind(gained)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// [`observe_percent_tx`] on its own transaction, for writers outside the
/// position upsert — the derived-percent attach, which is where the position
/// actually becomes measurable for a CFI-only client.
pub(super) async fn observe_percent(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    format: ProgressFormat,
    percent: i64,
    observed_at: i64,
) -> Result<(), ProgressError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    observe_percent_tx(&mut tx, user_id, book_uuid, format, percent, observed_at).await?;
    tx.commit().await?;
    Ok(())
}

/// The UTC day (`YYYY-MM-DD`) the ledger began recording, or `None` on a
/// database whose migration row is somehow absent. Everything read before it is
/// unrecoverable — there was no position trail to difference — which is why the
/// stats surfaces state the date rather than letting the tile change meaning
/// without saying so.
pub async fn pages_ledger_epoch(pool: &SqlitePool) -> Result<Option<String>, ProgressError> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
            .bind(PAGES_LEDGER_EPOCH_KEY)
            .fetch_optional(pool)
            .await?,
    )
}
