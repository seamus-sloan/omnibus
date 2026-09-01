//! Forward-progress ledger (migrations `0083`, `0092`): turns the mutable
//! position row into an append-only record of how much of a book was covered on
//! each day. `db::stats::pages` reads it; the position write path is the only
//! writer — every site that sets `reading_progress.progress_percent` must
//! observe here too, or the ground that write covered is lost for good.
//!
//! Accrual is scoped to a **sitting**, not to a single write, which is what
//! keeps a there-and-back move from being charged twice; see
//! [`observe_percent_tx`].

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

/// Record an observed whole-book percent for `(user, book, format)` and accrue
/// whatever ground it covers beyond the current sitting's furthest point into
/// that day's bucket.
///
/// The gain is measured against the ledger's own mark, never against
/// `reading_progress.progress_percent`: an epub write carrying only a CFI nulls
/// that column and `spawn_epub_percent_derivation` refills it off the request
/// path, so the live row is NULL for part of its life and differencing it would
/// silently lose every gain on the surfaces that write CFIs (iOS).
///
/// The mark is the **sitting's high-water percent**, not the last position seen
/// (migration `0092`). Differencing consecutive writes measured the path the
/// reader walked rather than the ground they covered, so flipping back to find
/// a quote and returning charged the stretch in between twice. Holding the
/// furthest point instead makes a there-and-back move free, while forward
/// reading the reader later backtracks out of stays credited — they did read
/// it.
///
/// Three cases accrue nothing on purpose:
///
/// * A book with no mark yet is *baselined* — a device syncing a book it is
///   already 60% through has not just read 60% of it.
/// * A first observation more than [`IDLE_GAP_SECS`] after the previous one
///   opens a **new sitting** and baselines again, wherever the reader now is.
///   That is what keeps a deliberate re-read counting in full: restarting a
///   finished book baselines at 5% rather than facing a lifetime high-water of
///   100 that would swallow the whole re-read.
/// * A move that stays at or below the sitting's high-water mark is ground
///   already credited.
///
/// `observed_at` is the surviving record's client event time, so a write
/// replayed from an offline outbox lands on the day it happened rather than the
/// day it drained. An observation that arrives *out of order* — the
/// off-request-path percent derivation completing after a newer write, carrying
/// the older position's event time — is treated as part of the sitting already
/// in progress rather than as a gap, and cannot drag the sitting clock
/// backward.
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

    sqlx::query(
        "INSERT INTO reading_progress_daily
             (user_id, book_uuid, format, day, percent_gained, updated_at)
         VALUES (?, ?, ?, date(?, 'unixepoch'), ?, ?)
         ON CONFLICT(user_id, book_uuid, day, format) DO UPDATE SET
             percent_gained = percent_gained + excluded.percent_gained,
             updated_at = excluded.updated_at",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(fmt)
    .bind(observed_at)
    .bind(gained)
    .bind(observed_at)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// The `(new sitting high-water, ground gained)` an observation produces
/// against an existing mark. Split out from [`observe_percent_tx`] so the whole
/// sitting rule reads as three lines in one place rather than interleaved with
/// the two statements that persist it.
///
/// `last_at` is NULL only for a row migration `0092` created before the sitting
/// clock existed and that nothing has observed since. It reads as "no sitting in
/// progress", which re-baselines — right for those rows, none of which is mid
/// sitting across a deploy.
fn sitting_gain(mark: i64, last_at: Option<i64>, percent: i64, observed_at: i64) -> (i64, i64) {
    // A negative gap is an out-of-order observation, not idleness, so it
    // continues the sitting rather than opening one — `<=` covers both.
    let continues = last_at.is_some_and(|last| observed_at - last <= IDLE_GAP_SECS);
    if !continues {
        return (percent, 0);
    }
    (mark.max(percent), (percent - mark).max(0))
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
