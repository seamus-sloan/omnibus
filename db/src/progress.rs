//! Server-authoritative reading/listening position sync plus batched
//! session reports. Position upserts are most-recent-wins on
//! `(user_id, book_uuid, format)` by client event time, not server receipt
//! time. Session inserts go to the per-format `reading_sessions` /
//! `listening_sessions` tables; all rows soft-reference `books.uuid`.

use std::collections::HashSet;
use std::sync::{Mutex, OnceLock, PoisonError};

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
pub async fn upsert_progress(
    pool: &SqlitePool,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    let mut tx = pool.begin().await?;
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
             COALESCE(reading_progress.client_updated_at, reading_progress.updated_at)",
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
        format: update.format,
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
        kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
        book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
        client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
    })
}

/// Attach a server-derived KoboSpan location (and percent, when the row
/// has none) to an existing epub row WITHOUT advancing any freshness
/// clock — the derived half describes the same position the row already
/// holds, so bumping `updated_at`/`client_updated_at` would re-fire the
/// Kobo sync delta forever. Optimistic: no-ops unless the row still has
/// no location and its event time is unchanged since the caller read it.
/// Returns whether a row was updated.
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
    let result = sqlx::query(
        "UPDATE reading_progress
            SET kobo_location = ?,
                progress_percent = COALESCE(progress_percent, ?)
          WHERE user_id = ? AND book_uuid = ? AND format = 'epub'
            AND kobo_location IS NULL
            AND COALESCE(client_updated_at, updated_at) = ?",
    )
    .bind(location_json)
    .bind(percent)
    .bind(user_id)
    .bind(&book_uuid)
    .bind(expected_client_updated_at)
    .execute(pool)
    .await?;
    Ok(result.rows_affected() > 0)
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
    Ok(result.rows_affected() > 0)
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
    let Some(source) = crate::book_file_path(pool, book_id, "EPUB").await? else {
        return Ok(false);
    };
    let cfi = cfi.to_owned();
    let derived =
        tokio::task::spawn_blocking(move || crate::kobo_position::cfi_to_span(None, &source, &cfi))
            .await
            .context("percent derivation task panicked")??;
    let Some(percent) = derived.percent else {
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
    let book_uuid = record.book_uuid.clone();
    let expected = record.client_updated_at;
    tokio::spawn(async move {
        let result = derive_epub_percent(&pool, user_id, &book_uuid, &cfi, expected).await;
        percent_derivations_in_flight()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        if let Err(e) = result {
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
/// truth. The stamp clamps forward to server-now like [`upsert_progress_tx`]'s.
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
    Ok(Some(ProgressRecord {
        book_uuid: canonical,
        format: parse_format(row.try_get::<String, _>("format")?.as_str()),
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
        kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
        book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
        client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
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

/// The user's most recent progress rows across both formats, newest first
/// by **client event time** — `COALESCE(client_updated_at, updated_at)`, so
/// a queued offline replay landing late doesn't jump a week-old book to the
/// top. Feeds the "pick up where you left off" surfaces via
/// [`resume_points`].
///
/// Books the user has marked `unread` or `finished` are excluded: both are
/// statements that they are not mid-book, and a completed book otherwise sat
/// at the top of the rail until five newer positions pushed it off.
///
/// A **missing** `book_read_status` row keeps its position. 0046 treats
/// absence as `unread` everywhere else, and this is the deliberate exception:
/// absence here means "nothing has been said", not "not reading". Every
/// status write is best-effort (`read_status_auto`, the players), so hiding on
/// absence would drop a book off the rail whenever one of those requests was
/// lost — and would hide every row written before that auto-transition
/// existed.
///
/// The join is per-book while progress is per-`(book, format)`, so finishing
/// a dual-format book clears both its reading and its listening card. Read
/// status is a fact about the book, not about one of its files.
pub async fn recent_progress(
    pool: &SqlitePool,
    user_id: i64,
    limit: i64,
) -> Result<Vec<ProgressRecord>, ProgressError> {
    let rows = sqlx::query(
        "SELECT rp.book_uuid, rp.format, rp.epub_cfi, rp.audio_position_seconds,
                rp.progress_percent, rp.kobo_location, rp.book_file_id,
                rp.updated_at,
                COALESCE(rp.client_updated_at, rp.updated_at) AS client_updated_at
         FROM reading_progress rp
         LEFT JOIN book_read_status rs
                ON rs.user_id = rp.user_id AND rs.book_uuid = rp.book_uuid
         WHERE rp.user_id = ?
           AND (rs.status IS NULL OR rs.status = 'reading')
         ORDER BY COALESCE(rp.client_updated_at, rp.updated_at) DESC, rp.book_uuid
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
                progress_percent: row.try_get::<Option<i64>, _>("progress_percent")?,
                kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
                book_file_id: row.try_get::<Option<i64>, _>("book_file_id")?,
                updated_at: row.try_get::<i64, _>("updated_at")?,
                client_updated_at: row.try_get::<i64, _>("client_updated_at")?,
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
    for mut record in records {
        let Some(book) = crate::get_book_by_uuid(pool, &record.book_uuid).await? else {
            continue;
        };
        let audio = match record.format {
            ProgressFormat::Audio => audio_totals(pool, &record.book_uuid, &record).await?,
            ProgressFormat::Epub => None,
        };
        let (total_duration_seconds, chapter_number, chapter_count) = match audio {
            Some(totals) => {
                // Overwrite rather than trust the stored id: it may name a
                // `book_files` row the reindex has since replaced, and the
                // Continue CTA links straight at `?file_id=` — a dead id
                // would open the player on a manifest that 404s.
                record.book_file_id = Some(totals.book_file_id);
                (
                    Some(totals.total_duration_seconds),
                    totals.chapter_number,
                    totals.chapter_count,
                )
            }
            None => {
                // Audio row whose book has no audio file left (and every
                // epub row): drop the stored id rather than hand a CTA an
                // id that resolves to nothing.
                record.book_file_id = None;
                (None, None, None)
            }
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

/// Which audio file a resume point plays, plus the duration and chapter
/// position measured against **that** file.
struct AudioTotals {
    book_file_id: i64,
    total_duration_seconds: f64,
    chapter_number: Option<i64>,
    chapter_count: Option<i64>,
}

/// Resolve the audio file for a progress row and measure duration + chapter
/// position against it. `None` when the book has no resolvable audio file
/// (e.g. every file was removed after the position was saved).
///
/// The row's stored `book_file_id` picks the file for a book carrying more
/// than one audiobook, so the resume card reads out the narration the user
/// was actually in. It is a soft reference (rule 06) — a stale id, or one
/// belonging to another book, falls back to the first audio file by ordinal,
/// which is what the whole feature did before the id was recorded.
async fn audio_totals(
    pool: &SqlitePool,
    uuid: &str,
    record: &ProgressRecord,
) -> Result<Option<AudioTotals>, ProgressError> {
    let stored = match record.book_file_id {
        Some(id) => hls::resolve_audiobook_file(pool, uuid, Some(id)).await?,
        None => None,
    };
    let resolved = match stored {
        Some(resolved) => resolved,
        None => match hls::resolve_audiobook(pool, uuid).await? {
            Some(resolved) => resolved,
            None => return Ok(None),
        },
    };
    let parts = hls::get_parts(pool, resolved.book_file_id).await?;
    let total: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    let mut chapters = hls::get_chapters(pool, resolved.book_file_id).await?;
    chapters.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));
    let position = record.audio_position_seconds.unwrap_or(0.0);
    Ok(Some(AudioTotals {
        book_file_id: resolved.book_file_id,
        total_duration_seconds: total,
        chapter_number: chapter_number_at(&chapters, position),
        chapter_count: (!chapters.is_empty()).then_some(chapters.len() as i64),
    }))
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
    let mut tx = pool.begin().await?;
    let result = record_session_tx(&mut tx, user_id, report).await?;
    tx.commit().await?;
    Ok(result)
}

#[cfg(test)]
mod tests;
