//! SQL-heavy aggregation body for [`super::user_stats`]: `compute` runs one
//! query per [`StatsSummary`] field and assembles the result. Sibling modules
//! hold the rollups that grew their own sub-topic — `genre` for the donut,
//! `pages` for the page estimate, `ratings` for the star metrics — keeping
//! each file under the house line-count cap.

use omnibus_shared::{
    DayActivity, FinishedBook, MonthCount, PeriodComparison, RankedEntity, StatsRange, StatsSummary,
};
use sqlx::{Row, SqlitePool};

use super::{genre, pages, ratings, sessionize, streak, StatsError};

/// How many rows the top-authors / top-tags rollups return.
const TOP_N: i64 = 8;

/// Cap on the finished-books rail. `books_finished` counts every completion
/// via [`finished_count`]; this only bounds the rendered list so an
/// `AllTime` range on a long-lived library can't return an unbounded vec.
pub(super) const FINISHED_BOOKS_LIMIT: i64 = 100;

/// The book-scoped session union reused by the top-N rollups: one
/// `(book_uuid, secs)` row per reading and listening session in the window.
/// Bind order is `user_id, start, user_id, start`.
pub(super) const SESSION_BOOK_SECS: &str = "\
    SELECT book_uuid, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND started_at >= ? \
    UNION ALL \
    SELECT book_uuid, seconds_listened AS secs FROM listening_sessions \
        WHERE user_id = ? AND started_at >= ?";

/// The time-scoped session union, reused by the busiest-week rollup and the
/// sitting count: one `(book_uuid, started_at, ended_at, secs)` checkpoint row
/// per session in the window. Bind order is `user_id, start, user_id, start`.
const SESSION_ROWS: &str = "\
    SELECT book_uuid, started_at, ended_at, seconds_read AS secs FROM reading_sessions \
        WHERE user_id = ? AND started_at >= ? \
    UNION ALL \
    SELECT book_uuid, started_at, ended_at, seconds_listened AS secs FROM listening_sessions \
        WHERE user_id = ? AND started_at >= ?";

/// Book-level completion events for a user, unioned across the two ways a book
/// can be "finished": a 100% journal entry (keyed on `created_at`) and an
/// explicit read-status `finished` (keyed on `finished_at`). One
/// `(book_uuid, finished_at)` row per source; callers window on `finished_at`
/// and `COUNT(DISTINCT book_uuid)` so a book finished both ways counts once.
/// Bind order is `user_id, user_id`.
pub(super) const FINISHED_EVENTS: &str = "\
    SELECT book_uuid, created_at AS finished_at FROM journal_entries \
        WHERE user_id = ? AND progress = 100 \
    UNION ALL \
    SELECT book_uuid, finished_at FROM book_read_status \
        WHERE user_id = ? AND status = 'finished' AND finished_at IS NOT NULL";

/// [`FINISHED_EVENTS`] narrowed to completions on a book that still exists —
/// the liveness filter **every** completion metric shares. Same
/// `(book_uuid, finished_at)` shape, same two binds.
///
/// Derived once rather than spelled out per query: several of these metrics
/// render on the same screen, so a metric that opts out of the filter has to
/// do so visibly.
fn live_finished_events() -> String {
    format!(
        "SELECT f.book_uuid AS book_uuid, f.finished_at AS finished_at \
         FROM ({FINISHED_EVENTS}) f \
         JOIN books b ON b.uuid = f.book_uuid"
    )
}

/// Run every per-field query and assemble the [`StatsSummary`] for one user's
/// window. This is the body [`super::user_stats_at`] caches.
pub(super) async fn compute(
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
    let avg_stars = ratings::avg_stars(pool, user_id, start).await?;
    let rating_histogram = ratings::rating_histogram(pool, user_id, start).await?;
    let heatmap = heatmap(pool, user_id, start).await?;
    // One clock read feeds both the heatmap's right edge and the streak's
    // anchor, so the two can never land on different days.
    let (as_of_day, today) = as_of(pool).await?;
    let streak = streak::streak(pool, user_id, start, today).await?;
    let (busiest_week_start, busiest_week_seconds) = busiest_week(pool, user_id, start).await?;
    let top_authors = top_authors(pool, user_id, start).await?;
    let top_tags = top_tags(pool, user_id, start).await?;
    let genre_share = genre::genre_share(pool, user_id, start).await?;
    let genre_tagged_books = genre::genre_tagged_books(pool, user_id, start).await?;
    let books_active = books_active(pool, user_id, start).await?;
    let finished_books = finished_books(pool, user_id, start).await?;
    let books_finished = finished_count(pool, user_id, start).await?;
    let books_per_month = books_per_month(pool, user_id).await?;
    let previous = previous_period(pool, user_id, range).await?;
    let listening_daily = listening_daily(pool, user_id, start).await?;
    let rating_monthly = ratings::rating_monthly(pool, user_id).await?;
    let pages_read = pages::pages_read(pool, user_id, start).await?;

    Ok(StatsSummary {
        range,
        reading_seconds,
        listening_seconds,
        avg_stars,
        sessions,
        active_days: streak.active_days,
        longest_streak_days: streak.longest_days,
        current_streak_days: streak.current_days,
        busiest_week_start,
        busiest_week_seconds,
        books_finished,
        books_active,
        as_of_day,
        heatmap,
        top_authors,
        top_tags,
        genre_share,
        genre_tagged_books,
        finished_books,
        books_per_month,
        previous,
        listening_daily,
        rating_monthly,
        rating_histogram,
        pages_read,
    })
}

/// SQLite expression for a range's period start. Calendar math (start-of-year,
/// month arithmetic) stays in SQLite rather than Rust; each arm is a fixed
/// literal, never user input.
///
/// Shared by [`window_start`] and [`prev_window_bounds`] so the current window
/// and the baseline it is compared against cannot drift onto different
/// definitions of where a period begins.
fn window_start_expr(range: StatsRange) -> &'static str {
    match range {
        // Rolling 7 calendar days ending today — deliberately not aligned
        // to a weekday, per the converged stats design.
        StatsRange::Week => "strftime('%s', 'now', '-6 days', 'start of day')",
        StatsRange::Month => "strftime('%s', strftime('%Y-%m-01 00:00:00', 'now'))",
        StatsRange::Year => "strftime('%s', strftime('%Y-01-01 00:00:00', 'now'))",
        StatsRange::AllTime => "0",
    }
}

/// Lower bound (unix secs, inclusive) of the reporting window.
pub(super) async fn window_start(pool: &SqlitePool, range: StatsRange) -> Result<i64, StatsError> {
    let expr = window_start_expr(range);
    Ok(
        sqlx::query_scalar(&format!("SELECT CAST({expr} AS INTEGER)"))
            .fetch_one(pool)
            .await?,
    )
}

pub(super) async fn sum_seconds(
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

/// Sittings in the window, counted over [`sessionize::stitched`] groups
/// rather than raw checkpoint rows — the figure would otherwise report a
/// client's flush cadence (~60 rows for an hour on web, ~12 on iOS) instead
/// of how often the user actually sat down with a book. Stitching is
/// per-book, so this stays the sum of every book's Pickups, and glances under
/// [`sessionize::MIN_SITTING_SECS`] don't count (their seconds still do,
/// in `reading_seconds` / `listening_seconds`).
///
/// Rows are windowed before they are stitched, so a sitting straddling
/// `start` is truncated to its in-window part rather than pulling pre-window
/// time into the count — and can fall under the floor once truncated.
pub(super) async fn session_count(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COUNT(*) FROM ({}) WHERE secs >= {}",
        sessionize::stitched(SESSION_ROWS),
        sessionize::MIN_SITTING_SECS
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

pub(super) async fn heatmap(
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

/// The busiest ISO week: `(first active day, total seconds)`. Weeks bucket by
/// Monday — `dnum - ((dnum + 3) % 7)`, since unix day 0 (1970-01-01) is a
/// Thursday. Returns `(None, 0)` when the window has no sessions.
pub(super) async fn busiest_week(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<(Option<String>, i64), StatsError> {
    let sql = format!(
        "SELECT MIN(day) AS first_day, SUM(secs) AS seconds FROM (
             SELECT date(started_at, 'unixepoch') AS day,
                    (started_at / 86400) - (((started_at / 86400) + 3) % 7) AS week_start,
                    secs
             FROM ({SESSION_ROWS})
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

pub(super) async fn top_authors(
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

pub(super) async fn top_tags(
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

/// Distinct books with any session activity in the window.
pub(super) async fn books_active(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<i64, StatsError> {
    let sql = format!("SELECT COUNT(DISTINCT book_uuid) FROM ({SESSION_BOOK_SECS})");
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// The server's current UTC day as both the `YYYY-MM-DD` string stamped on the
/// summary — so the heatmap grid anchors to the server clock instead of the
/// client's — and its unix day number, which anchors the current streak.
///
/// Both come out of **one** statement on purpose: SQLite holds `'now'` fixed
/// for a single statement, so the two can't straddle midnight and describe
/// different days.
pub(super) async fn as_of(pool: &SqlitePool) -> Result<(String, i64), StatsError> {
    let row = sqlx::query(
        "SELECT date('now') AS day, CAST(strftime('%s', 'now') AS INTEGER) / 86400 AS dnum",
    )
    .fetch_one(pool)
    .await?;
    Ok((row.get("day"), row.get("dnum")))
}

/// Total completions in the window under the same definition as
/// [`finished_books`], uncapped — the rail is limited to
/// [`FINISHED_BOOKS_LIMIT`] rows but `books_finished` must reflect the
/// real count. Counts distinct live books finished via either a 100% journal
/// entry or an explicit read-status `finished` (see [`FINISHED_EVENTS`]).
pub(super) async fn finished_count(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COUNT(DISTINCT f.book_uuid) FROM ({}) f WHERE f.finished_at >= ?",
        live_finished_events()
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// Books completed in the window — sourced from either a 100% journal entry or
/// an explicit read-status `finished` (see [`FINISHED_EVENTS`]). A book finished
/// both ways collapses to one row with the newest completion moment. Ghosted
/// books (no live `books` row for the `book_uuid`) are omitted from the rail and
/// the count. Capped at [`FINISHED_BOOKS_LIMIT`] newest completions.
pub(super) async fn finished_books(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<FinishedBook>, StatsError> {
    let sql = format!(
        "SELECT b.uuid AS uuid,
                COALESCE(b.title, 'Untitled') AS title,
                a.name AS author,
                MAX(f.finished_at) AS finished_at,
                MAX(b.has_cover) AS has_cover,
                MAX(ur.half_stars) AS half_stars
         FROM ({FINISHED_EVENTS}) f
         JOIN books b ON b.uuid = f.book_uuid
         LEFT JOIN books_authors_link bal ON bal.book = b.id AND bal.position = 0
         LEFT JOIN authors a ON a.id = bal.author
         LEFT JOIN user_ratings ur ON ur.user_id = ? AND ur.book_uuid = b.uuid
         WHERE f.finished_at >= ?
         GROUP BY b.uuid
         ORDER BY finished_at DESC
         LIMIT ?"
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .bind(FINISHED_BOOKS_LIMIT)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let uuid: String = r.get("uuid");
            let has_cover: i64 = r.get("has_cover");
            let half_stars: Option<i64> = r.get("half_stars");
            FinishedBook {
                cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
                rating: half_stars
                    .map(|h| f64::from(u32::try_from(h.clamp(0, 10)).unwrap_or(0)) / 2.0),
                book_uuid: uuid,
                title: r.get("title"),
                author: r.get("author"),
                finished_at: r.get("finished_at"),
            }
        })
        .collect())
}

/// Bounds `(start, end)` (unix secs, `start` inclusive, `end` exclusive) of the
/// baseline the current window is compared against: the **same elapsed slice**
/// of the preceding period. `None` for [`StatsRange::AllTime`] — there is no
/// window before "all of it".
///
/// The current window is period-to-date, so the baseline has to be the same
/// offset into the previous period for the two to be commensurable. `end` is
/// clamped to the current period's start, since the elapsed offset can exceed
/// a shorter previous period (30 days into March against a 28-day February).
pub(super) async fn prev_window_bounds(
    pool: &SqlitePool,
    range: StatsRange,
) -> Result<Option<(i64, i64)>, StatsError> {
    let prev_start_expr = match range {
        StatsRange::Week => "strftime('%s', 'now', '-13 days', 'start of day')",
        // Month arithmetic runs on a month-start anchor: applying '-1 month'
        // to a month-end 'now' (e.g. July 31) normalizes to day 1 of the
        // *current* month and collapses the window.
        StatsRange::Month => "strftime('%s', 'now', 'start of month', '-1 month')",
        StatsRange::Year => "strftime('%s', strftime('%Y-01-01 00:00:00', 'now', '-1 year'))",
        StatsRange::AllTime => return Ok(None),
    };
    let cur_start_expr = window_start_expr(range);
    // One statement, so `now` is read from the same clock tick as both period
    // starts and the elapsed offset can't straddle a second boundary.
    let row = sqlx::query(&format!(
        "SELECT CAST({prev_start_expr} AS INTEGER) AS prev_start,
                CAST({cur_start_expr} AS INTEGER) AS cur_start,
                CAST(strftime('%s', 'now') AS INTEGER) AS now_secs"
    ))
    .fetch_one(pool)
    .await?;
    Ok(Some(prev_window_from(
        row.get("prev_start"),
        row.get("cur_start"),
        row.get("now_secs"),
    )))
}

/// The bounds arithmetic of [`prev_window_bounds`], split from its clock so it
/// can be tested on fixed dates — reading SQLite's `now` makes the clamp
/// reachable on only a handful of days a year.
///
/// Yields `[prev_start, prev_start + elapsed)`, clamped to `cur_start` so the
/// baseline never runs past the period it belongs to. `elapsed` is zero for the
/// first second of a period, which yields an empty baseline — correct, since
/// the current window is equally empty then.
pub(super) fn prev_window_from(prev_start: i64, cur_start: i64, now: i64) -> (i64, i64) {
    let elapsed = now.saturating_sub(cur_start).max(0);
    (
        prev_start,
        prev_start.saturating_add(elapsed).min(cur_start),
    )
}

/// The window immediately preceding `range`'s current one — feeds the drill-in's
/// vs-previous-period delta. Zeroed for [`StatsRange::AllTime`] (no prior window).
pub(super) async fn previous_period(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
) -> Result<PeriodComparison, StatsError> {
    let Some((start, end)) = prev_window_bounds(pool, range).await? else {
        return Ok(PeriodComparison::default());
    };
    let listening_seconds = sum_seconds_bounded(
        pool,
        user_id,
        start,
        end,
        "listening_sessions",
        "seconds_listened",
    )
    .await?;
    let avg_stars = ratings::avg_stars_bounded(pool, user_id, start, end).await?;
    let books_finished = finished_count_bounded(pool, user_id, start, end).await?;
    Ok(PeriodComparison {
        books_finished,
        avg_stars,
        listening_seconds,
    })
}

/// `sum_seconds`, upper-bounded — `started_at` in `[start, end)`.
async fn sum_seconds_bounded(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    end: i64,
    table: &str,
    col: &str,
) -> Result<i64, StatsError> {
    // `table` / `col` are fixed literals chosen by the caller, never user input.
    let sql = format!(
        "SELECT COALESCE(SUM({col}), 0) FROM {table} \
         WHERE user_id = ? AND started_at >= ? AND started_at < ?"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_one(pool)
        .await?)
}

/// Count of distinct live books finished (either source, see
/// [`FINISHED_EVENTS`]) with the completion moment in `[start, end)`. Shares
/// [`finished_count`]'s definition exactly, so the drill-in's delta compares
/// two counts of the same thing.
async fn finished_count_bounded(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    end: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COUNT(DISTINCT f.book_uuid) FROM ({}) f
         WHERE f.finished_at >= ? AND f.finished_at < ?",
        live_finished_events()
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_one(pool)
        .await?)
}

/// Daily listening seconds within the window — the Listening tile's drill-in
/// trend chart. Mirrors [`heatmap`] but scoped to `listening_sessions` alone.
pub(super) async fn listening_daily(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<DayActivity>, StatsError> {
    let rows = sqlx::query(
        "SELECT date(started_at, 'unixepoch') AS day, SUM(seconds_listened) AS seconds
         FROM listening_sessions WHERE user_id = ? AND started_at >= ?
         GROUP BY day ORDER BY day",
    )
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

/// Books finished per calendar month over the trailing 12 months (oldest
/// first, ending at the current month), independent of any windowing —
/// the all-time trend chart is never scoped to the period switcher. Uses the
/// same completion definition as [`finished_books`] (either source, see
/// [`FINISHED_EVENTS`]). A recursive CTE generates the 12-month spine and
/// `LEFT JOIN`s it against the unified completion events in one query, so a
/// month with no finishes still comes back as zero rather than being omitted,
/// and a 10k-event library never pays an N+1.
pub(super) async fn books_per_month(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<MonthCount>, StatsError> {
    let sql = format!(
        "WITH RECURSIVE months(month) AS (
             SELECT strftime('%Y-%m', 'now', 'start of month', '-11 months')
             UNION ALL
             SELECT strftime('%Y-%m', month || '-01', '+1 month')
             FROM months
             WHERE month < strftime('%Y-%m', 'now')
         )
         SELECT months.month AS month, COUNT(DISTINCT f.book_uuid) AS books
         FROM months
         LEFT JOIN ({}) f
               ON strftime('%Y-%m', f.finished_at, 'unixepoch') = months.month
         GROUP BY months.month
         ORDER BY months.month",
        live_finished_events()
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| MonthCount {
            month: r.get("month"),
            books: r.get("books"),
        })
        .collect())
}
