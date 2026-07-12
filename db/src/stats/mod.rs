//! Reading-stats aggregation over the `reading_sessions` /
//! `listening_sessions` tables plus `journal_entries` for completion; no new
//! schema. Every metric is computed in SQL. Results sit behind a process-wide
//! per-user cache with a 60s TTL so a reload reflects a just-finished session
//! while poll-driven refreshes inside the window skip the SQL.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use omnibus_shared::{DayActivity, FinishedBook, RankedEntity, StatsRange, StatsSummary};
use sqlx::{Row, SqlitePool};

#[cfg(test)]
mod tests;

/// Per-user aggregate cache TTL. A reload after a just-finished session
/// reflects new data within this window; repeated calls inside it hit the
/// cache instead of re-running the SQL.
pub const STATS_TTL_SECS: i64 = 60;

/// How many rows the top-authors / top-tags rollups return.
const TOP_N: i64 = 8;

/// The book-scoped session union reused by the top-N rollups: one
/// `(book_uuid, secs)` row per reading and listening session in the window.
/// Bind order is `user_id, start, user_id, start`.
const SESSION_BOOK_SECS: &str = "\
    SELECT book_uuid, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND started_at >= ? \
    UNION ALL \
    SELECT book_uuid, seconds_listened FROM listening_sessions \
        WHERE user_id = ? AND started_at >= ?";

/// The time-scoped session union reused by the busiest-week rollup: one
/// `(started_at, secs)` row per session in the window. Bind order is
/// `user_id, start, user_id, start`.
const SESSION_TIME_SECS: &str = "\
    SELECT started_at, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND started_at >= ? \
    UNION ALL \
    SELECT started_at, seconds_listened FROM listening_sessions \
        WHERE user_id = ? AND started_at >= ?";

/// Failure space of the aggregation layer. Wraps `sqlx::Error` at the module
/// boundary so no raw DB error leaks to callers.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

type Cache = Mutex<HashMap<(i64, StatsRange), (i64, StatsSummary)>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Aggregate one user's reading/listening stats over `range`, cached for
/// [`STATS_TTL_SECS`].
pub async fn user_stats(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
) -> Result<StatsSummary, StatsError> {
    user_stats_at(pool, user_id, range, now_secs()).await
}

/// Clock-injected core of [`user_stats`]. A cache hit fresher than the TTL
/// returns without touching the DB; otherwise the SQL runs and the result is
/// cached under `(user_id, range)` stamped at `now`.
async fn user_stats_at(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
    now: i64,
) -> Result<StatsSummary, StatsError> {
    if let Some(hit) = cache_get(user_id, range, now) {
        return Ok(hit);
    }
    let summary = compute(pool, user_id, range).await?;
    cache_put(user_id, range, now, summary.clone());
    Ok(summary)
}

fn cache_get(user_id: i64, range: StatsRange, now: i64) -> Option<StatsSummary> {
    let guard = cache().lock().ok()?;
    let (at, summary) = guard.get(&(user_id, range))?;
    (now.saturating_sub(*at) < STATS_TTL_SECS).then(|| summary.clone())
}

fn cache_put(user_id: i64, range: StatsRange, now: i64, summary: StatsSummary) {
    if let Ok(mut guard) = cache().lock() {
        guard.insert((user_id, range), (now, summary));
    }
}

/// Drop every cached entry. Test-only: the cache is a process-wide `static`, so
/// a test exercising the cached path clears it first to stay order-independent.
#[cfg(test)]
pub(crate) fn clear_cache() {
    if let Ok(mut guard) = cache().lock() {
        guard.clear();
    }
}

async fn compute(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
) -> Result<StatsSummary, StatsError> {
    let start = window_start(pool, range).await?;
    let reading_seconds =
        sum_seconds(pool, user_id, start, "reading_sessions", "seconds_read").await?;
    let listening_seconds = sum_seconds(
        pool,
        user_id,
        start,
        "listening_sessions",
        "seconds_listened",
    )
    .await?;
    let sessions = session_count(pool, user_id, start).await?;
    let avg_stars = avg_stars(pool, user_id, start).await?;
    let heatmap = heatmap(pool, user_id, start).await?;
    let (active_days, longest_streak_days) = streak(pool, user_id, start).await?;
    let (busiest_week_start, busiest_week_seconds) = busiest_week(pool, user_id, start).await?;
    let top_authors = top_authors(pool, user_id, start).await?;
    let top_tags = top_tags(pool, user_id, start).await?;
    let finished_books = finished_books(pool, user_id, start).await?;

    Ok(StatsSummary {
        range,
        reading_seconds,
        listening_seconds,
        avg_stars,
        sessions,
        active_days,
        longest_streak_days,
        busiest_week_start,
        busiest_week_seconds,
        books_finished: finished_books.len() as i64,
        heatmap,
        top_authors,
        top_tags,
        finished_books,
    })
}

/// Lower bound (unix secs, inclusive) of the reporting window. Computed with
/// SQLite date functions so calendar math (start-of-year, month arithmetic)
/// stays out of Rust. The `expr` is a fixed literal per range — no user input.
async fn window_start(pool: &SqlitePool, range: StatsRange) -> Result<i64, StatsError> {
    let expr = match range {
        // Rolling 7 calendar days ending today — deliberately not aligned
        // to a weekday, per the converged stats design.
        StatsRange::Week => "strftime('%s', 'now', '-6 days', 'start of day')",
        StatsRange::Month => "strftime('%s', strftime('%Y-%m-01 00:00:00', 'now'))",
        StatsRange::Year => "strftime('%s', strftime('%Y-01-01 00:00:00', 'now'))",
        StatsRange::AllTime => "0",
    };
    Ok(
        sqlx::query_scalar(&format!("SELECT CAST({expr} AS INTEGER)"))
            .fetch_one(pool)
            .await?,
    )
}

async fn sum_seconds(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    table: &str,
    col: &str,
) -> Result<i64, StatsError> {
    // `table` / `col` are fixed literals chosen by the caller, never user input.
    let sql = format!(
        "SELECT COALESCE(SUM({col}), 0) FROM {table} WHERE user_id = ? AND started_at >= ?"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// Mean star rating over books the user rated within the window (keyed on
/// `user_ratings.updated_at`), in stars — `half_stars` is 1..=10, so the
/// SQL mean halves it. `None` when nothing was rated in the window.
async fn avg_stars(pool: &SqlitePool, user_id: i64, start: i64) -> Result<Option<f64>, StatsError> {
    Ok(sqlx::query_scalar(
        "SELECT AVG(half_stars) / 2.0 FROM user_ratings
         WHERE user_id = ? AND updated_at >= ?",
    )
    .bind(user_id)
    .bind(start)
    .fetch_one(pool)
    .await?)
}

async fn session_count(pool: &SqlitePool, user_id: i64, start: i64) -> Result<i64, StatsError> {
    Ok(sqlx::query_scalar(
        "SELECT
           (SELECT COUNT(*) FROM reading_sessions   WHERE user_id = ? AND started_at >= ?)
         + (SELECT COUNT(*) FROM listening_sessions WHERE user_id = ? AND started_at >= ?)",
    )
    .bind(user_id)
    .bind(start)
    .bind(user_id)
    .bind(start)
    .fetch_one(pool)
    .await?)
}

async fn heatmap(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<DayActivity>, StatsError> {
    let rows = sqlx::query(
        "SELECT day, SUM(secs) AS seconds FROM (
             SELECT date(started_at, 'unixepoch') AS day, seconds_read AS secs
                 FROM reading_sessions   WHERE user_id = ? AND started_at >= ?
             UNION ALL
             SELECT date(started_at, 'unixepoch'), seconds_listened
                 FROM listening_sessions WHERE user_id = ? AND started_at >= ?
         ) GROUP BY day ORDER BY day",
    )
    .bind(user_id)
    .bind(start)
    .bind(user_id)
    .bind(start)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| DayActivity {
            day: r.get("day"),
            seconds: r.get("seconds"),
        })
        .collect())
}

/// Active-day count and longest consecutive-day streak. Days are unix day
/// numbers (`started_at / 86400`, UTC) so consecutiveness is an integer diff.
async fn streak(pool: &SqlitePool, user_id: i64, start: i64) -> Result<(i64, i64), StatsError> {
    let days: Vec<i64> = sqlx::query_scalar(
        "SELECT DISTINCT started_at / 86400 AS dnum FROM (
             SELECT started_at FROM reading_sessions   WHERE user_id = ? AND started_at >= ?
             UNION ALL
             SELECT started_at FROM listening_sessions WHERE user_id = ? AND started_at >= ?
         ) ORDER BY dnum",
    )
    .bind(user_id)
    .bind(start)
    .bind(user_id)
    .bind(start)
    .fetch_all(pool)
    .await?;

    let active_days = days.len() as i64;
    let mut longest = if days.is_empty() { 0 } else { 1 };
    let mut run = longest;
    for pair in days.windows(2) {
        run = if pair[1] - pair[0] == 1 { run + 1 } else { 1 };
        longest = longest.max(run);
    }
    Ok((active_days, longest))
}

/// The busiest ISO week: `(first active day, total seconds)`. Weeks bucket by
/// Monday — `dnum - ((dnum + 3) % 7)`, since unix day 0 (1970-01-01) is a
/// Thursday. Returns `(None, 0)` when the window has no sessions.
async fn busiest_week(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<(Option<String>, i64), StatsError> {
    let sql = format!(
        "SELECT MIN(day) AS first_day, SUM(secs) AS seconds FROM (
             SELECT date(started_at, 'unixepoch') AS day,
                    (started_at / 86400) - (((started_at / 86400) + 3) % 7) AS week_start,
                    secs
             FROM ({SESSION_TIME_SECS})
         ) GROUP BY week_start ORDER BY seconds DESC, week_start ASC LIMIT 1"
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_optional(pool)
        .await?;

    Ok(match row {
        Some(r) => (r.get("first_day"), r.get("seconds")),
        None => (None, 0),
    })
}

async fn top_authors(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<RankedEntity>, StatsError> {
    let sql = format!(
        "SELECT a.name AS name, SUM(x.secs) AS seconds FROM ({SESSION_BOOK_SECS}) x
             JOIN books b ON b.uuid = x.book_uuid
             JOIN books_authors_link bal ON bal.book = b.id AND bal.position = 0
             JOIN authors a ON a.id = bal.author
         GROUP BY a.id ORDER BY seconds DESC, a.name ASC LIMIT ?"
    );
    ranked(pool, &sql, user_id, start).await
}

async fn top_tags(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<RankedEntity>, StatsError> {
    let sql = format!(
        "SELECT t.name AS name, SUM(x.secs) AS seconds FROM ({SESSION_BOOK_SECS}) x
             JOIN books b ON b.uuid = x.book_uuid
             JOIN books_tags_link btl ON btl.book = b.id
             JOIN tags t ON t.id = btl.tag
         GROUP BY t.id ORDER BY seconds DESC, t.name ASC LIMIT ?"
    );
    ranked(pool, &sql, user_id, start).await
}

async fn ranked(
    pool: &SqlitePool,
    sql: &str,
    user_id: i64,
    start: i64,
) -> Result<Vec<RankedEntity>, StatsError> {
    let rows = sqlx::query(sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .bind(TOP_N)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| RankedEntity {
            name: r.get("name"),
            seconds: r.get("seconds"),
        })
        .collect())
}

/// Books completed in the window — sourced from `journal_entries.progress = 100`
/// (the only progress fraction persisted today). Ghosted books (no live `books`
/// row for the journal's `book_uuid`) are omitted from the rail and the count.
async fn finished_books(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<FinishedBook>, StatsError> {
    let rows = sqlx::query(
        "SELECT b.uuid AS uuid,
                COALESCE(b.title, 'Untitled') AS title,
                a.name AS author,
                MAX(j.created_at) AS finished_at
         FROM journal_entries j
         JOIN books b ON b.uuid = j.book_uuid
         LEFT JOIN books_authors_link bal ON bal.book = b.id AND bal.position = 0
         LEFT JOIN authors a ON a.id = bal.author
         WHERE j.user_id = ? AND j.progress = 100 AND j.created_at >= ?
         GROUP BY b.uuid
         ORDER BY finished_at DESC",
    )
    .bind(user_id)
    .bind(start)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|r| FinishedBook {
            book_uuid: r.get("uuid"),
            title: r.get("title"),
            author: r.get("author"),
            finished_at: r.get("finished_at"),
        })
        .collect())
}
