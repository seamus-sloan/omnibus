//! Star-rating aggregation for the stats page's Avg rating tile and its
//! drill-in: the window mean, the previous window's mean, the trailing-12-month
//! mean trend, and the half-star distribution. Every function here shares one
//! window key, [`avg_stars`]'s — they must not diverge on it.

use omnibus_shared::{RatingBucket, TrendPoint};
use sqlx::{Row, SqlitePool};

use super::StatsError;

/// A rating on a book that still exists, scoped to one user — the same
/// liveness rule `compute::live_finished_events` applies to completions.
/// Bind order is `user_id`.
///
/// A rating the book page cannot render must not move the mean.
const LIVE_RATINGS: &str = "\
    SELECT r.user_id AS user_id, r.half_stars AS half_stars, r.updated_at AS updated_at \
    FROM user_ratings r \
    JOIN books b ON b.uuid = r.book_uuid \
    WHERE r.user_id = ?";

/// Mean star rating over books the user rated within the window, in stars —
/// `half_stars` is 1..=10, so the SQL mean halves it. `None` when nothing was
/// rated in the window. Live books only, per [`LIVE_RATINGS`].
///
/// **The window key is `user_ratings.updated_at` — when the reader rated the
/// book, not when they finished it**, and every other function in this module
/// follows. Keying on the completion event would drop every rating on a book
/// carrying no completion event at all (abandoned and rated, or rated without
/// ever being marked finished), which is a worse distortion than the one it
/// fixes. The consequence to know: re-rating a book read years ago pulls it
/// into the current window, and a book finished this month but rated last
/// month falls out. Whichever key is in force, the mean and the distribution
/// under it must share it or they describe different sets of books.
pub(super) async fn avg_stars(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<f64>, StatsError> {
    let sql = format!("SELECT AVG(half_stars) / 2.0 FROM ({LIVE_RATINGS}) WHERE updated_at >= ?");
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// [`avg_stars`], upper-bounded — `updated_at` in `[start, end)`.
pub(super) async fn avg_stars_bounded(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    end: i64,
) -> Result<Option<f64>, StatsError> {
    let sql = format!(
        "SELECT AVG(half_stars) / 2.0 FROM ({LIVE_RATINGS}) \
         WHERE updated_at >= ? AND updated_at < ?"
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_one(pool)
        .await?)
}

/// How many books sit in each half-star bucket over the window — the shape
/// [`avg_stars`] flattens away. A reader who rates everything 4 and one who
/// splits evenly between 2 and 5 both report a mean of 3.5.
///
/// All ten buckets come back, zeros included: a histogram with missing bars
/// reads as a different distribution than the one it describes. The spine is a
/// recursive CTE `LEFT JOIN`ed against the ratings, same shape as
/// `compute::books_per_month`'s month spine.
///
/// Scoped identically to [`avg_stars`] — same `updated_at` window, same
/// [`LIVE_RATINGS`] liveness filter — so the bucket counts sum to exactly the
/// set of ratings the mean is computed over.
pub(super) async fn rating_histogram(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<RatingBucket>, StatsError> {
    let sql = format!(
        "WITH RECURSIVE buckets(half_stars) AS (
             SELECT 1
             UNION ALL
             SELECT half_stars + 1 FROM buckets WHERE half_stars < 10
         )
         SELECT buckets.half_stars AS half_stars, COUNT(r.half_stars) AS books
         FROM buckets
         LEFT JOIN ({LIVE_RATINGS}) r
                ON r.half_stars = buckets.half_stars AND r.updated_at >= ?
         GROUP BY buckets.half_stars
         ORDER BY buckets.half_stars"
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| RatingBucket {
            half_stars: r.get("half_stars"),
            books: r.get("books"),
        })
        .collect())
}

/// Mean star rating per calendar month over the trailing 12 months (oldest
/// first, ending at the current month) — the Avg rating tile's drill-in trend
/// chart. Same trailing-window CTE shape as `compute::books_per_month`; a month
/// with no ratings comes back as `0.0` rather than being omitted.
pub(super) async fn rating_monthly(
    pool: &SqlitePool,
    user_id: i64,
) -> Result<Vec<TrendPoint>, StatsError> {
    let sql = format!(
        "WITH RECURSIVE months(month) AS (
             SELECT strftime('%Y-%m', 'now', 'start of month', '-11 months')
             UNION ALL
             SELECT strftime('%Y-%m', month || '-01', '+1 month')
             FROM months
             WHERE month < strftime('%Y-%m', 'now')
         )
         SELECT months.month AS month, AVG(ur.half_stars) / 2.0 AS avg_stars
         FROM months
         LEFT JOIN ({LIVE_RATINGS}) ur
                ON strftime('%Y-%m', ur.updated_at, 'unixepoch') = months.month
         GROUP BY months.month
         ORDER BY months.month"
    );
    let rows = sqlx::query(&sql).bind(user_id).fetch_all(pool).await?;

    Ok(rows
        .into_iter()
        .map(|r| {
            let month: String = r.get("month");
            let avg_stars: Option<f64> = r.get("avg_stars");
            TrendPoint {
                label: month,
                value: avg_stars.unwrap_or(0.0),
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
