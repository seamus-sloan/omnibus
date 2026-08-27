//! Per-book insights aggregation for the book-detail page's stats stop —
//! Started / Time read / Pickups / Longest sit plus the per-day activity
//! spark, scoped to one `(user, book_uuid)` rather than
//! [`super::user_stats`]'s user-wide window. Same `reading_sessions` /
//! `listening_sessions` tables, no new query-time schema.

use omnibus_shared::{BookInsights, DayActivity};
use sqlx::{Row, SqlitePool};

use super::{sessionize, StatsError};

/// One `(book_uuid, started_at, ended_at, secs)` union of reading + listening
/// checkpoint rows scoped to a single book. Bind order is
/// `user_id, book_uuid, user_id, book_uuid`.
const BOOK_SESSIONS: &str = "\
    SELECT book_uuid, started_at, ended_at, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND book_uuid = ? \
    UNION ALL \
    SELECT book_uuid, started_at, ended_at, seconds_listened AS secs FROM listening_sessions \
        WHERE user_id = ? AND book_uuid = ?";

/// Aggregate one user's reading/listening insights for a single book:
/// earliest session start, total seconds and sitting count across both
/// formats, the single longest sitting (length + when it started), and
/// per-day activity for the spark. `uuid` is resolved to the canonical
/// `books.uuid` first (mirroring `db::progress`'s write path), so a link into
/// a book that was later merged away still finds the sessions recorded under
/// the surviving book. Returns `None` when the uuid doesn't resolve to a live
/// book, or the book resolves but has no sessions yet — both drive the stats
/// stop's empty state.
///
/// Every count here is over [`sessionize::stitched`] sittings rather than raw
/// checkpoint rows, so the figures don't vary with the reporting client's
/// flush cadence. The per-day spark stays on the raw rows — a sitting that
/// crosses midnight belongs to both days.
///
/// `sessions` counts only sittings of at least
/// [`sessionize::MIN_SITTING_SECS`], while `seconds_total` sums every row the
/// stitch grouped. A book whose only activity is glances therefore reports
/// zero sessions and takes the empty state, the same as one never opened.
pub async fn book_insights(
    pool: &SqlitePool,
    user_id: i64,
    uuid: &str,
) -> Result<Option<BookInsights>, StatsError> {
    let Some(book_uuid) = crate::books::resolve_canonical_book_uuid(pool, uuid).await? else {
        return Ok(None);
    };
    let sittings = sessionize::stitched(BOOK_SESSIONS);
    let min_secs = sessionize::MIN_SITTING_SECS;

    let sql = format!(
        "SELECT MIN(started_at) AS started_at, COALESCE(SUM(secs), 0) AS seconds_total, \
                COALESCE(SUM(CASE WHEN secs >= {min_secs} THEN 1 ELSE 0 END), 0) AS sessions \
         FROM ({sittings})"
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(&book_uuid)
        .bind(user_id)
        .bind(&book_uuid)
        .fetch_one(pool)
        .await?;

    let sessions: i64 = row.get("sessions");
    if sessions == 0 {
        return Ok(None);
    }
    let started_at: i64 = row.get("started_at");
    let seconds_total: i64 = row.get("seconds_total");

    // Longest single sit. Ties break to the earliest occurrence so the
    // answer is stable across runs. The floor is redundant against a `MAX`
    // — the early return above already proved a qualifying sitting exists,
    // and it is the longest — but stated so this stays a query over the same
    // population `sessions` counted.
    let sql = format!(
        "SELECT started_at, secs FROM ({sittings}) \
         WHERE secs >= {min_secs} ORDER BY secs DESC, started_at ASC LIMIT 1"
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(&book_uuid)
        .bind(user_id)
        .bind(&book_uuid)
        .fetch_one(pool)
        .await?;
    let longest_seconds: i64 = row.get("secs");
    let longest_started_at: i64 = row.get("started_at");

    // Per-day totals, same UTC bucketing as the user-wide heatmap. Active
    // days only — the client fills calendar gaps against `as_of_day`.
    let sql = format!(
        "SELECT date(started_at, 'unixepoch') AS day, SUM(secs) AS seconds \
         FROM ({BOOK_SESSIONS}) GROUP BY day ORDER BY day"
    );
    let daily = sqlx::query(&sql)
        .bind(user_id)
        .bind(&book_uuid)
        .bind(user_id)
        .bind(&book_uuid)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(|r| DayActivity {
            day: r.get("day"),
            seconds: r.get("seconds"),
        })
        .collect();

    Ok(Some(BookInsights {
        started_at,
        seconds_total,
        sessions,
        longest_seconds,
        longest_started_at,
        daily,
        as_of_day: super::compute::as_of_day(pool).await?,
    }))
}

#[cfg(test)]
mod tests;
