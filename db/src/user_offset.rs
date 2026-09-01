//! The reader's UTC offset — the single calendar every day-boundary figure in
//! `crate::stats` is cut on.
//!
//! A day is *ordinal*: today, yesterday, seven in a row. A sequence whose
//! elements are measured on different calendars cannot be ordered, so every
//! figure with a day boundary — the heatmap, the streak, both daily goals, the
//! Week/Month/Year window edges — resolves against **one** offset per request:
//! the one the asking client declares, falling back to the reader's most recent
//! session and finally to UTC.
//!
//! Which *hour* a session sits in is a different question and takes a different
//! answer — the offset that session recorded at capture time, so an evening read
//! in Tokyo stays an evening after the reader flies home. That lives in
//! `crate::stats::patterns` and is deliberately not this.

use omnibus_shared::SessionReport;
use sqlx::{Row, SqlitePool};

#[cfg(test)]
mod tests;

/// Failure space of the offset lookups: each is a query, so a wrapped
/// `sqlx::Error` is the only way they fail. An enum rather than a bare
/// `sqlx::Error` so no raw DB error crosses the module boundary.
#[derive(Debug, thiserror::Error)]
pub enum OffsetError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

/// The offset this request's day boundaries are cut on, in minutes east of UTC.
///
/// Prefers what the asking client declared — it is the only source that knows
/// where the reader is *now*, which is what a day boundary is a fact about.
/// Falls back to the offset of their most recent session, then to UTC.
///
/// Returns a concrete offset rather than an `Option` because the caller always
/// has to bucket on something; `0` is what a day gets called when nobody can say
/// better, not a claim that the reader is at UTC.
///
/// An out-of-range claim is discarded rather than rejected. The offset shifts a
/// timestamp before a date is read off it, so an absurd value does not produce
/// an obviously bad row — it silently files reading on the wrong day, and every
/// caller here is a read path that should degrade rather than 500.
pub async fn resolve_offset_minutes(
    pool: &SqlitePool,
    user_id: i64,
    claimed: Option<i64>,
) -> Result<i64, OffsetError> {
    if let Some(offset) = claimed.filter(|o| in_range(*o)) {
        return Ok(offset);
    }
    Ok(current_offset_minutes(pool, user_id).await?.unwrap_or(0))
}

/// Whether `offset` is a real IANA offset — UTC-12:00 through UTC+14:00. Bounds
/// shared with [`SessionReport::validate`] so the API boundary and the read path
/// cannot come to disagree about what a plausible offset is.
fn in_range(offset: i64) -> bool {
    (SessionReport::UTC_OFFSET_MIN_MINUTES..=SessionReport::UTC_OFFSET_MAX_MINUTES)
        .contains(&offset)
}

/// The reader's UTC offset from the most recent session carrying one, or `None`
/// when no session ever has.
///
/// A **proxy**, and worth naming as one: it says where they were when they last
/// read, not where they are now, and it is stale by an hour after a DST
/// transition the reader has not read across. Only reached when the asking
/// client declared no offset of its own — the sessions' `time_zone` column
/// (migration `0092`) is what would answer this exactly, once there is a tz
/// database to resolve it against.
///
/// One indexed probe per table, compared here, rather than one `ORDER BY` over a
/// `UNION ALL` of both: SQLite cannot push the sort through the union, so that
/// shape materialises and sorts a reader's whole offset-carrying session history
/// on a path that runs for every stats load. Each probe instead walks
/// `idx_{reading,listening}_sessions_user_started` backwards and stops at the
/// first row it wants.
pub async fn current_offset_minutes(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Option<i64>, OffsetError> {
    let mut latest: Option<(i64, i64)> = None;
    for table in ["reading_sessions", "listening_sessions"] {
        // `table` is one of two fixed literals chosen here, never user input.
        let sql = format!(
            "SELECT started_at, utc_offset_minutes FROM {table}
             WHERE user_id = ? AND utc_offset_minutes IS NOT NULL
             ORDER BY started_at DESC LIMIT 1"
        );
        let Some(row) = sqlx::query(&sql).bind(user_id).fetch_optional(pool).await? else {
            continue;
        };
        let candidate: (i64, i64) = (
            row.try_get("started_at")?,
            row.try_get("utc_offset_minutes")?,
        );
        if latest.is_none_or(|(seen, _)| candidate.0 > seen) {
            latest = Some(candidate);
        }
    }
    // Bounds-checked on the way out as well as at the API boundary: this reads
    // rows written before that check existed, and one bad session must not
    // relabel a reader's days.
    Ok(latest.map(|(_, offset)| offset).filter(|o| in_range(*o)))
}

/// Today's date as `YYYY-MM-DD` under `offset_minutes`.
///
/// Read from SQLite rather than the process clock so it lands on the same "now"
/// every other window boundary in `crate::stats` is cut on.
pub async fn today(pool: &SqlitePool, offset_minutes: i64) -> Result<String, OffsetError> {
    Ok(sqlx::query_scalar(
        "SELECT date(CAST(strftime('%s','now') AS INTEGER) + ? * 60, 'unixepoch')",
    )
    .bind(offset_minutes)
    .fetch_one(pool)
    .await?)
}
