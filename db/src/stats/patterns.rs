//! Time-of-day and day-of-week rollups for [`super::user_stats`]: how the
//! window's reading and listening seconds distribute across the 24 hours of a
//! day and the 7 days of a week. Both are zero-filled to their full width, so
//! the shape of a day stays legible instead of collapsing to the hours that
//! happened to have activity.
//!
//! # Timezone
//!
//! **These buckets are local-time, resolved from the UTC offset each session
//! recorded at capture time (`SessionReport::utc_offset_minutes`, migration
//! 0080).** Every other rollup in `db::stats` buckets in UTC, which for a
//! calendar-day heatmap is a rounding error a reader never notices. For an
//! hour-of-day chart it is the whole signal: a reader at UTC-7 who reads at
//! 21:00 would be reported reading at 04:00, and the chart would not be
//! slightly off, it would be wrong.
//!
//! Of the three ways to get a local answer, this one is the only retroactively
//! honest one. A stored per-user timezone re-labels a session with wherever
//! the account is *now*, so a fortnight read in Tokyo becomes a fortnight of
//! 4am reading the moment the reader flies home. Bucketing client-side puts
//! web, iOS, and any widget on three independent derivations of one chart —
//! the disagreement the server-computed fields on `StatsSummary` exist to
//! prevent. Recording where the device was keeps a Tokyo evening a Tokyo
//! evening, and keeps the bucketing in one place.
//!
//! **Rows with no offset are excluded, not defaulted.** Sessions written
//! before migration 0080 carry no record of where the reader was; stamping
//! them UTC would invent a fact, and stamping them with a later-observed
//! offset would invent a different one. Their seconds come back as
//! [`TimePatterns::unzoned_seconds`] so the surfaces can disclose that the
//! strips cover less than the window's total rather than quietly under-report.
//! A stored per-user timezone would compose here as a fallback for exactly
//! those rows; nothing else in this module would change.
//!
//! Only these two charts are local-time. The heatmap, streaks, active days,
//! and the period boundaries themselves remain UTC — that is a broader fix
//! than this module's scope.

use omnibus_shared::{HourBucket, WeekdayBucket};
use sqlx::{Row, SqlitePool};

use super::StatsError;

/// Hours in a day / days in a week — the fixed widths both strips zero-fill
/// to.
const HOURS: usize = 24;
const WEEKDAYS: usize = 7;

/// Weekday column labels, Monday first. The server owns these (rather than
/// letting each client name index 0) because week-start is a convention: a
/// client assuming Sunday-first would draw every column one place out and
/// nothing would fail.
const WEEKDAY_LABELS: [&str; WEEKDAYS] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

/// The session union these rollups share: one `(started_at, offset, secs)`
/// row per reading and listening session in the window, so both formats are
/// counted the same way every other activity metric counts them. Bind order
/// is `user_id, start, user_id, start`.
///
/// Checkpoint rows are bucketed as they were written, and that is correct
/// here: the web tracker's `rollover()` restarts a segment every 60s, so a sit
/// spanning an hour boundary already arrives as two rows and lands its
/// minutes on both sides of the boundary. A later change that coalesces
/// checkpoints back into whole sittings must not reattribute a sitting's whole
/// duration to the hour it *started* in — that would quietly pile a 90-minute
/// evening onto 21:00 and leave 22:00 empty.
const SESSION_LOCAL_ROWS: &str = "\
    SELECT started_at, utc_offset_minutes, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND started_at >= ? \
    UNION ALL \
    SELECT started_at, utc_offset_minutes, seconds_listened FROM listening_sessions \
        WHERE user_id = ? AND started_at >= ?";

/// Both time-pattern strips plus the seconds neither could place.
#[derive(Debug)]
pub(super) struct TimePatterns {
    /// All 24 local hours, ascending, zeros included.
    pub hour_of_day: Vec<HourBucket>,
    /// All 7 local weekdays, Monday first, zeros included.
    pub day_of_week: Vec<WeekdayBucket>,
    /// Seconds in the window from sessions carrying no capture-time offset.
    pub unzoned_seconds: i64,
}

/// Roll the window's sessions up by local hour of day and local weekday.
///
/// One `GROUP BY` over the shared union produces both: the same rows feed
/// each, so the two strips can never describe different sets of sessions.
/// Sessions with no recorded offset are summed separately into
/// [`TimePatterns::unzoned_seconds`] rather than being bucketed — see the
/// module docs.
pub(super) async fn time_patterns(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<TimePatterns, StatsError> {
    let (hours, weekdays) = zoned_buckets(pool, user_id, start).await?;
    Ok(TimePatterns {
        hour_of_day: hours
            .iter()
            .enumerate()
            .map(|(hour, &seconds)| HourBucket {
                // 0..=23 by construction.
                hour: hour as i64,
                seconds,
            })
            .collect(),
        day_of_week: weekdays
            .iter()
            .enumerate()
            .map(|(weekday, &seconds)| WeekdayBucket {
                // 0..=6 by construction.
                weekday: weekday as i64,
                label: WEEKDAY_LABELS[weekday].to_string(),
                seconds,
            })
            .collect(),
        unzoned_seconds: unzoned_seconds(pool, user_id, start).await?,
    })
}

/// The two zero-filled tallies, keyed by local hour and Monday-first weekday.
///
/// SQLite does the shift and the calendar work: `started_at + offset * 60`
/// moved into `'unixepoch'` is the device's own wall clock, and `%H` / `%w`
/// read the hour and weekday straight off it. Doing the shift on the
/// *timestamp* rather than rotating an aggregate afterwards is what keeps the
/// half- and quarter-hour zones (UTC+05:30, UTC+05:45) exact — a rotation of
/// 24 whole-hour buckets cannot represent them.
async fn zoned_buckets(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<([i64; HOURS], [i64; WEEKDAYS]), StatsError> {
    let sql = format!(
        "SELECT CAST(strftime('%H', started_at + utc_offset_minutes * 60, 'unixepoch')
                     AS INTEGER) AS hour,
                CAST(strftime('%w', started_at + utc_offset_minutes * 60, 'unixepoch')
                     AS INTEGER) AS dow,
                SUM(secs) AS seconds
         FROM ({SESSION_LOCAL_ROWS})
         WHERE utc_offset_minutes IS NOT NULL
         GROUP BY hour, dow"
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?;

    let mut hours = [0_i64; HOURS];
    let mut weekdays = [0_i64; WEEKDAYS];
    for row in rows {
        let seconds: i64 = row.get("seconds");
        // `strftime` yields NULL for a timestamp it can't render; a row that
        // can't be placed on a clock is dropped rather than folded into
        // midnight-Monday, which is a real bucket a reader would believe.
        let (Ok(hour), Ok(dow)) = (row.try_get::<i64, _>("hour"), row.try_get::<i64, _>("dow"))
        else {
            continue;
        };
        if let Some(slot) = usize::try_from(hour).ok().and_then(|h| hours.get_mut(h)) {
            *slot += seconds;
        }
        // SQLite's `%w` is 0 = Sunday; the wire order is Monday first.
        if let Some(slot) = usize::try_from((dow + 6) % 7)
            .ok()
            .and_then(|d| weekdays.get_mut(d))
        {
            *slot += seconds;
        }
    }
    Ok((hours, weekdays))
}

/// Seconds in the window from sessions that recorded no UTC offset — the
/// total the two strips are *not* drawn over.
async fn unzoned_seconds(pool: &SqlitePool, user_id: i64, start: i64) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COALESCE(SUM(secs), 0) FROM ({SESSION_LOCAL_ROWS}) \
         WHERE utc_offset_minutes IS NULL"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

#[cfg(test)]
mod tests;
