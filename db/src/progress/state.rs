//! Per-book progress state outside the upsert conflict path: the
//! server-derived Kobo location attach, the derived whole-book percent
//! attach, the mirrored `Statistics` block, the plain position getter, and
//! the audiobook playback-rate preference.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, BookProgress, ProgressFormat,
    ProgressRecord,
};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{resolve_canonical_book_uuid, resolve_canonical_book_uuid_exec};

use super::enrich::{enrich_record, PositionDetail};
use super::{format_str, ledger, parse_format, ProgressError};

/// Attach a server-derived KoboSpan location (and percent, when the row
/// has none) to an existing epub row WITHOUT advancing any freshness
/// clock — the derived half describes the same position the row already
/// holds, so bumping `updated_at`/`client_updated_at` would re-fire the
/// Kobo sync delta forever. Optimistic: no-ops unless the row still has
/// no location and its event time is unchanged since the caller read it.
/// Returns whether a row was updated.
///
/// A percent this fills is observed by the forward-progress ledger, like every
/// other writer of that column — see [`attach_derived_percent`].
pub async fn attach_derived_kobo_location(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    location_json: &str,
    percent: Option<i64>,
    expected_client_updated_at: i64,
) -> Result<bool, ProgressError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let attached: Option<Option<i64>> = sqlx::query_scalar(
        "UPDATE reading_progress
            SET kobo_location = ?,
                progress_percent = COALESCE(progress_percent, ?)
          WHERE user_id = ? AND book_uuid = ? AND format = 'epub'
            AND kobo_location IS NULL
            AND COALESCE(client_updated_at, updated_at) = ?
      RETURNING progress_percent",
    )
    .bind(location_json)
    .bind(percent)
    .bind(user_id)
    .bind(&book_uuid)
    .bind(expected_client_updated_at)
    .fetch_optional(pool)
    .await?;
    // This is the third writer of `progress_percent`, and the ledger has to see
    // all three or it silently under-counts: a CFI-only write ledgers nothing,
    // and whichever of this attach and `attach_derived_percent` lands first
    // makes the other a no-op. `RETURNING` gives the percent the row actually
    // ended up with rather than the one offered, so a `COALESCE` that kept an
    // existing value re-observes that value — which the mark has already seen,
    // so it accrues nothing — instead of dragging the mark to a lower one.
    if let Some(Some(settled)) = attached {
        ledger::observe_percent(
            pool,
            user_id,
            &book_uuid,
            ProgressFormat::Epub,
            settled,
            expected_client_updated_at,
        )
        .await?;
    }
    Ok(attached.is_some())
}

/// Attach a server-derived whole-book percent to an existing epub row
/// WITHOUT advancing any freshness clock — the same contract as
/// [`attach_derived_kobo_location`]: the percent describes the position the
/// row already holds, so bumping a clock would re-fire sync deltas.
/// Optimistic: no-ops unless the row still has no percent and its event
/// time is unchanged since the caller read it. Returns whether a row was
/// updated.
pub async fn attach_derived_percent(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    percent: i64,
    expected_client_updated_at: i64,
) -> Result<bool, ProgressError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let result = sqlx::query(
        "UPDATE reading_progress
            SET progress_percent = ?
          WHERE user_id = ? AND book_uuid = ? AND format = 'epub'
            AND progress_percent IS NULL
            AND COALESCE(client_updated_at, updated_at) = ?",
    )
    .bind(percent)
    .bind(user_id)
    .bind(&book_uuid)
    .bind(expected_client_updated_at)
    .execute(pool)
    .await?;
    let attached = result.rows_affected() > 0;
    if attached {
        // For a CFI-only client (the iOS reader) this attach — not the upsert —
        // is where the position first becomes a percent, so it is the only
        // place the forward-progress ledger can see it. Stamped with the
        // position's own event time, since the derivation runs off the request
        // path and its completion moment says nothing about when the reading
        // happened.
        ledger::observe_percent(
            pool,
            user_id,
            &book_uuid,
            ProgressFormat::Epub,
            percent,
            expected_client_updated_at,
        )
        .await?;
    }
    Ok(attached)
}

/// Derive the whole-book visible-text percent for a stored epub CFI from
/// the book's source EPUB alone (no kepub involved) and attach it via
/// [`attach_derived_percent`]. `Ok(false)` covers every underivable case —
/// no EPUB file row, an unparseable CFI, an unanchorable tail, or a row
/// whose event time moved on since `expected_client_updated_at` was read —
/// mirroring `kobo_position`'s rule of degrading to no derived value,
/// never a wrong one. `Err` is reserved for I/O-level surprises.
pub async fn derive_epub_percent(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    cfi: &str,
    expected_client_updated_at: i64,
) -> anyhow::Result<bool> {
    use anyhow::Context;

    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(false);
    };
    let Some((file_id, source)) = crate::book_file_with_id(pool, book_id, "EPUB").await? else {
        return Ok(false);
    };
    // Stored spine stats (migration 0074) reduce the derivation to one
    // spine-document walk plus arithmetic; a book the backfill hasn't
    // reached yet falls back to the full-book walk. Both paths share the
    // same normalization and floor semantics, so they cannot disagree.
    let stats = crate::epub_structure::get_spine_stats(pool, file_id)
        .await
        .map_err(|e| anyhow::anyhow!("read spine stats for file {file_id}: {e}"))?;
    let cfi = cfi.to_owned();
    let percent = if stats.is_empty() {
        tokio::task::spawn_blocking(move || crate::kobo_position::cfi_to_span(None, &source, &cfi))
            .await
            .context("percent derivation task panicked")??
            .percent
    } else {
        tokio::task::spawn_blocking(move || crate::kobo_position::cfi_spine_offset(&source, &cfi))
            .await
            .context("percent derivation task panicked")??
            .and_then(|(spine_index, offset)| {
                crate::epub_structure::percent_at(&stats, spine_index as i64, offset)
            })
    };
    let Some(percent) = percent else {
        return Ok(false);
    };
    match attach_derived_percent(
        pool,
        user_id,
        book_uuid,
        percent,
        expected_client_updated_at,
    )
    .await
    {
        Ok(updated) => Ok(updated),
        // The book vanished between the upsert and the attach; nothing left
        // to annotate is a non-event, not a failure.
        Err(ProgressError::BookNotFound) => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// In-flight guard for [`spawn_epub_percent_derivation`]: one spine walk per
/// `(user, book)` at a time, so a page-turn burst costs one walk. Skipped
/// writes self-heal — the next accepted write re-spawns after the running
/// walk finishes.
fn percent_derivations_in_flight() -> &'static Mutex<HashSet<(i64, String)>> {
    static IN_FLIGHT: OnceLock<Mutex<HashSet<(i64, String)>>> = OnceLock::new();
    IN_FLIGHT.get_or_init(Mutex::default)
}

/// Removes its key from the in-flight set on drop, so the slot frees even
/// when the derivation future panics or is dropped mid-flight — a leaked
/// key would suppress every later derivation for that `(user, book)` until
/// process restart.
struct InFlightGuard {
    key: (i64, String),
}

impl Drop for InFlightGuard {
    fn drop(&mut self) {
        percent_derivations_in_flight()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&self.key);
    }
}

/// Fire-and-forget percent derivation for an accepted epub write that landed
/// without one (the web reader sends only a CFI). Runs off the request path:
/// the caller responds immediately and the percent attaches clock-neutrally
/// when the walk completes. No-op for audio rows, rows already carrying a
/// percent (Kobo bookmarks, comic anchors), and CFI-less rows.
pub fn spawn_epub_percent_derivation(pool: SqlitePool, user_id: i64, record: &ProgressRecord) {
    if record.format != ProgressFormat::Epub || record.progress_percent.is_some() {
        return;
    }
    let Some(cfi) = record.epub_cfi.clone() else {
        return;
    };
    let key = (user_id, record.book_uuid.clone());
    {
        let mut in_flight = percent_derivations_in_flight()
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if !in_flight.insert(key.clone()) {
            return;
        }
    }
    // The guard exists from here on, so the key leaves the set no matter
    // how the future ends — completion, panic, or being dropped unrun.
    let guard = InFlightGuard { key };
    let book_uuid = record.book_uuid.clone();
    let expected = record.client_updated_at;
    tokio::spawn(async move {
        let _guard = guard;
        if let Err(e) = derive_epub_percent(&pool, user_id, &book_uuid, &cfi, expected).await {
            tracing::warn!(%book_uuid, error = %e, "epub percent derivation failed");
        }
    });
}

/// The device's own `Statistics` counters, mirrored so sync-out can hand
/// them back. Never derived, never aggregated — see
/// [`set_kobo_statistics_tx`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KoboStatistics {
    pub spent_reading_minutes: Option<i64>,
    pub remaining_time_minutes: Option<i64>,
    /// The device's `Statistics.LastModified`, clamped forward to server-now.
    /// `None` when the device sent counters but no parseable clock.
    pub updated_at: Option<i64>,
}

impl KoboStatistics {
    /// Whether both counters are absent — nothing worth storing or echoing.
    pub fn is_empty(&self) -> bool {
        self.spent_reading_minutes.is_none() && self.remaining_time_minutes.is_none()
    }
}

/// Mirror a device's `Statistics` block onto its epub position row (#1653);
/// returns whether a row was updated.
///
/// UPDATE, not upsert: statistics annotate a position, they don't create one.
/// A stamped write wins over an older or unstamped stored block (an unstamped
/// one can't be echoed anyway); an unstamped write only takes an empty slot.
/// So a second Kobo can't overwrite newer totals and have them echoed back as
/// truth. The stamp clamps forward to server-now like
/// [`upsert_progress_tx`](super::upsert::upsert_progress_tx)'s.
pub async fn set_kobo_statistics_tx(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    book_uuid: &str,
    stats: &KoboStatistics,
) -> Result<bool, ProgressError> {
    let book_uuid = resolve_canonical_book_uuid_exec(&mut **tx, book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    // A NULL stamp stays NULL rather than becoming now: an invented clock
    // would win the device's arbitration and overwrite its own totals.
    let result = sqlx::query(
        "UPDATE reading_progress
            SET kobo_spent_reading_minutes = ?,
                kobo_remaining_time_minutes = ?,
                kobo_statistics_updated_at = CASE
                    WHEN ? IS NULL THEN NULL
                    ELSE MIN(?, CAST(strftime('%s','now') AS INTEGER))
                END
          WHERE user_id = ? AND book_uuid = ? AND format = 'epub'
            AND (
                (kobo_spent_reading_minutes IS NULL
                 AND kobo_remaining_time_minutes IS NULL
                 AND kobo_statistics_updated_at IS NULL)
             OR (? IS NOT NULL AND ? >= COALESCE(kobo_statistics_updated_at, 0))
            )",
    )
    .bind(stats.spent_reading_minutes)
    .bind(stats.remaining_time_minutes)
    .bind(stats.updated_at)
    .bind(stats.updated_at)
    .bind(user_id)
    .bind(&book_uuid)
    .bind(stats.updated_at)
    .bind(stats.updated_at)
    .execute(&mut **tx)
    .await?;
    Ok(result.rows_affected() > 0)
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
        "SELECT format, epub_cfi, audio_position_seconds, progress_percent, kobo_location,
                book_file_id, updated_at,
                COALESCE(client_updated_at, updated_at) AS client_updated_at
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
    Ok(Some(row_to_record(canonical, &row)?))
}

/// Decode one `reading_progress` row. The read-path enrichment fields start
/// empty — `enrich_record` fills them for the callers that serve them.
fn row_to_record(
    book_uuid: String,
    row: &sqlx::sqlite::SqliteRow,
) -> Result<ProgressRecord, ProgressError> {
    Ok(ProgressRecord {
        book_uuid,
        format: parse_format(row.try_get::<String, _>("format")?.as_str()),
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
        kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
        book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
        client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
        total_duration_seconds: None,
        resolved: None,
    })
}

/// Every position the user holds in one book, enriched with the runtime and
/// chapter data a caller would otherwise have to reconstruct.
///
/// `format` narrows the result to one record; omitted, **every** format the
/// user has a position in comes back. That default is the point: reading only
/// the epub row of a book the reader is 87% through in audio reports the
/// wrong place with nothing to signal it, and [`BookProgress::furthest`]
/// names the right one outright.
///
/// `Ok(None)` when the uuid names no book. A known book the user has never
/// opened returns an envelope with no records — "no position" and "no book"
/// are different answers.
pub async fn book_progress(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    format: Option<ProgressFormat>,
) -> Result<Option<BookProgress>, ProgressError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let mut records = read_records(pool, user_id, &canonical, format).await?;
    // `Full`: one book, asked for deliberately — the caller wants the most
    // precise answer available, and the archive walk is bounded by the number
    // of formats the reader holds a position in.
    for record in records.iter_mut() {
        enrich_record(pool, record, PositionDetail::Full).await?;
    }
    // Newest event time first, so a caller that ignores `furthest` and takes
    // the head still gets the most recently touched format rather than
    // whatever order SQLite happened to return.
    records.sort_by_key(|r| std::cmp::Reverse((r.client_updated_at, r.updated_at)));
    let furthest = furthest_of(&records);
    let linked = crate::cross_format::get_link(pool, user_id, &canonical)
        .await
        .map_err(cross_format_err)?
        .is_some();
    // Measured from the reader's true place, not from an arbitrary row: the
    // candidate answers "where would I pick this up in the other format",
    // which is only meaningful relative to where they actually are.
    let cross_format = match (linked, furthest) {
        (true, Some(from)) => {
            let target = match from {
                ProgressFormat::Epub => ProgressFormat::Audio,
                ProgressFormat::Audio => ProgressFormat::Epub,
            };
            match crate::cross_format::resume_candidate(pool, user_id, &canonical, target).await {
                Ok(resume) => resume.candidate,
                // A book that vanished mid-read, or an audio set that no
                // longer matches the one the link was confirmed against, is a
                // missing candidate — not a failed request.
                Err(
                    crate::cross_format::CrossFormatError::BookNotFound
                    | crate::cross_format::CrossFormatError::AudioSetMismatch,
                ) => None,
                Err(e) => return Err(cross_format_err(e)),
            }
        }
        _ => None,
    };
    Ok(Some(BookProgress {
        book_uuid: canonical,
        records,
        furthest,
        linked,
        cross_format,
    }))
}

/// Which record represents the reader's true place.
///
/// Whole-book percent decides it, because that is the question — a reader is
/// further along in the format they have covered more of, whichever they
/// touched last. The comparison needs a percent on **every** record to mean
/// anything, so a set with any missing one falls back to the most recent
/// event time rather than ranking a known percent against an assumed zero.
fn furthest_of(records: &[ProgressRecord]) -> Option<ProgressFormat> {
    fn percent(r: &ProgressRecord) -> Option<i64> {
        r.resolved
            .as_ref()
            .and_then(|res| res.percent_through_book)
            .or(r.progress_percent)
    }
    if records.iter().all(|r| percent(r).is_some()) {
        records
            .iter()
            .max_by_key(|r| (percent(r).unwrap_or(0), r.client_updated_at, r.updated_at))
            .map(|r| r.format)
    } else {
        records
            .iter()
            .max_by_key(|r| (r.client_updated_at, r.updated_at))
            .map(|r| r.format)
    }
}

/// The stored rows for one book, optionally narrowed to a single format.
/// Positions only — enrichment is the caller's job.
async fn read_records(
    pool: &SqlitePool,
    user_id: i64,
    canonical: &str,
    format: Option<ProgressFormat>,
) -> Result<Vec<ProgressRecord>, ProgressError> {
    let mut sql = String::from(
        "SELECT format, epub_cfi, audio_position_seconds, progress_percent, kobo_location,
                book_file_id, updated_at,
                COALESCE(client_updated_at, updated_at) AS client_updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_uuid = ?",
    );
    if format.is_some() {
        sql.push_str(" AND format = ?");
    }
    let mut q = sqlx::query(&sql).bind(user_id).bind(canonical);
    if let Some(f) = format {
        q = q.bind(format_str(f));
    }
    q.fetch_all(pool)
        .await?
        .into_iter()
        .map(|row| row_to_record(canonical.to_string(), &row))
        .collect()
}

/// Narrow a cross-format read error into this module's error space. The
/// declaration-only refusals are unreachable from the reads above but are
/// folded rather than panicked on, so a later caller can't open a silent path.
fn cross_format_err(e: crate::cross_format::CrossFormatError) -> ProgressError {
    match e {
        crate::cross_format::CrossFormatError::BookNotFound
        | crate::cross_format::CrossFormatError::AudioSetMismatch
        | crate::cross_format::CrossFormatError::LinkRequired
        | crate::cross_format::CrossFormatError::CounterpartMissing => ProgressError::BookNotFound,
        crate::cross_format::CrossFormatError::Sqlx(inner) => ProgressError::Sqlx(inner),
    }
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
