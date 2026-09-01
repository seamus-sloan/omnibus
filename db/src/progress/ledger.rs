//! Forward-progress ledger (migrations `0083`, `0093`, `0095`): turns the
//! mutable position row into an append-only record of how much of a book was
//! covered, and when — scoped to a sitting rather than to a single write.
//! `db::stats::pages` reads it; every site that sets
//! `reading_progress.progress_percent` must observe here too, or the ground
//! that write covered is lost for good.
//!
//! Two questions, decided in different places. **How much** to accrue is
//! settled here, against the sitting's high-water mark (`0093`). **Which day**
//! it lands on is not: gains accrue into quarter-hour slots (`0095`) and the day
//! is resolved on the way out, against whatever offset the reading client is on
//! — see `crate::user_offset`. Writing a day here, as `0083` did, is what made
//! the daily pages goal reset at UTC midnight.

use omnibus_shared::ProgressFormat;
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::stats::sessionize::IDLE_GAP_SECS;

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
/// whatever ground it covers beyond the current sitting's furthest point into
/// the quarter-hour it was observed in.
///
/// The gain is measured against the ledger's own mark, never against
/// `reading_progress.progress_percent`: an epub write carrying only a CFI nulls
/// that column and `spawn_epub_percent_derivation` refills it off the request
/// path, so the live row is NULL for part of its life and differencing it would
/// silently lose every gain on the surfaces that write CFIs (iOS).
///
/// The mark is the **sitting's high-water percent**, not the last position seen
/// (migration `0095`), so a there-and-back move inside one sitting is free
/// while forward ground the reader later backtracks out of stays credited.
///
/// [`IDLE_GAP_SECS`] of quiet opens a new sitting, and that governs the **mark**
/// alone, never the gain: it is the only thing that lets the mark fall back, so
/// a re-read counts from wherever it restarts instead of climbing to a lifetime
/// high-water of 100. Ground beyond the mark is earned either way — an offline
/// stretch drains as one coalesced write long after the fact, and it is real
/// reading, not a gap.
///
/// Two cases accrue nothing on purpose:
///
/// * A book with no mark yet is *baselined* — a device syncing a book it is
///   already 60% through has not just read 60% of it.
/// * A move at or below the mark is ground already credited.
///
/// `observed_at` is the surviving record's client event time, so a write
/// replayed from an offline outbox lands in the quarter-hour it happened in
/// rather than the one it drained in. An observation that arrives *out of
/// order* — the off-request-path percent derivation completing after a newer
/// write, carrying the older position's event time — is treated as part of the
/// sitting already in progress rather than as a gap, and cannot drag the
/// sitting clock backward.
pub(super) async fn observe_percent_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    book_uuid: &str,
    format: ProgressFormat,
    percent: i64,
    observed_at: i64,
) -> Result<(), ProgressError> {
    let fmt = format_str(format);
    let previous = sqlx::query(
        "SELECT sitting_max_percent, sitting_observed_at FROM reading_progress_marks
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

    let (sitting_max, gained) = match previous {
        Some(row) => {
            let mark: i64 = row.try_get("sitting_max_percent")?;
            let last_at: Option<i64> = row.try_get("sitting_observed_at")?;
            sitting_gain(mark, last_at, percent, observed_at)
        }
        // First observation of this book: baseline, accrue nothing.
        None => (percent, 0),
    };

    sqlx::query(
        "INSERT INTO reading_progress_marks
             (user_id, book_uuid, format, sitting_max_percent, sitting_observed_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?)
         ON CONFLICT(user_id, book_uuid, format) DO UPDATE SET
             sitting_max_percent = excluded.sitting_max_percent,
             -- Clamped forward so an out-of-order observation can't rewind the
             -- sitting clock and make the *next* one look like a fresh sitting.
             sitting_observed_at = MAX(
                 COALESCE(reading_progress_marks.sitting_observed_at, excluded.sitting_observed_at),
                 excluded.sitting_observed_at
             ),
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .bind(sitting_max)
    .bind(observed_at)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;

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

/// The `(new sitting high-water, ground gained)` an observation produces
/// against an existing mark.
///
/// `last_at` is NULL for a mark whose sitting was deliberately ended without
/// one being started — `merge::transaction::clear_sitting_clock` is the only
/// writer that does so. It reads as "no sitting in progress", which re-baselines.
fn sitting_gain(mark: i64, last_at: Option<i64>, percent: i64, observed_at: i64) -> (i64, i64) {
    // A negative gap is an out-of-order observation, not idleness, so it
    // continues the sitting rather than opening one — `<=` covers both.
    let continues = last_at.is_some_and(|last| observed_at - last <= IDLE_GAP_SECS);
    // Ground beyond the mark is always earned, boundary or not. A new sitting
    // that opens *ahead* of the mark is a reader who covered that stretch while
    // nothing was observing — both outboxes coalesce an offline stretch into a
    // single write stamped with the last page turn — and zeroing it there would
    // report a plane ride as no pages at all.
    let gained = (percent - mark).max(0);
    // The boundary decides only whether the mark may ratchet *down*. Inside a
    // sitting it never does, which is what makes a there-and-back move free;
    // opening a new one re-baselines, which is what lets a re-read count from
    // wherever it restarts instead of climbing back to a lifetime high-water.
    let next_mark = if continues {
        mark.max(percent)
    } else {
        percent
    };
    (next_mark, gained)
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
