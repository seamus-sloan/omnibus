//! Per-book insights aggregation for the book-detail page's stats stop —
//! Started / Time read / Pickups / Longest sit plus the per-day activity
//! spark, scoped to one `(user, book_uuid)` rather than
//! [`super::user_stats`]'s user-wide window. Same `reading_sessions` /
//! `listening_sessions` tables, no new query-time schema.

use omnibus_shared::{BookInsights, DayActivity};
use sqlx::{Row, SqlitePool};

use super::StatsError;

/// One `(started_at, secs)` union of reading + listening sessions scoped to a
/// single book. Bind order is `user_id, book_uuid, user_id, book_uuid`.
const BOOK_SESSIONS: &str = "\
    SELECT started_at, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND book_uuid = ? \
    UNION ALL \
    SELECT started_at, seconds_listened AS secs FROM listening_sessions \
        WHERE user_id = ? AND book_uuid = ?";

/// Aggregate one user's reading/listening insights for a single book:
/// earliest session start, total seconds and session count across both
/// formats, the single longest session (length + when it started), and
/// per-day activity for the spark. `uuid` is resolved to the canonical
/// `books.uuid` first (mirroring `db::progress`'s write path), so a link into
/// a book that was later merged away still finds the sessions recorded under
/// the surviving book. Returns `None` when the uuid doesn't resolve to a live
/// book, or the book resolves but has no sessions yet — both drive the stats
/// stop's empty state.
pub async fn book_insights(
    pool: &SqlitePool,
    user_id: i64,
    uuid: &str,
) -> Result<Option<BookInsights>, StatsError> {
    let Some(book_uuid) = crate::books::resolve_canonical_book_uuid(pool, uuid).await? else {
        return Ok(None);
    };

    let sql = format!(
        "SELECT MIN(started_at) AS started_at, COALESCE(SUM(secs), 0) AS seconds_total, \
                COUNT(*) AS sessions \
         FROM ({BOOK_SESSIONS})"
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
    // answer is stable across runs.
    let sql = format!(
        "SELECT started_at, secs FROM ({BOOK_SESSIONS}) \
         ORDER BY secs DESC, started_at ASC LIMIT 1"
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
