//! Per-book progress state outside the upsert conflict path: the
//! server-derived Kobo location attach, the mirrored `Statistics` block,
//! the plain position getter, and the audiobook playback-rate preference.

use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, ProgressFormat, ProgressRecord,
};
use sqlx::{Row, Sqlite, SqlitePool, Transaction};

use crate::{resolve_canonical_book_uuid, resolve_canonical_book_uuid_exec};

use super::{format_str, parse_format, ProgressError};

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
