//! Reading-stats aggregation over the `reading_sessions` /
//! `listening_sessions` tables plus `journal_entries` / `book_read_status`
//! for completion; no new query-time schema. See the `book`, `compute`, and
//! `pages` submodules for the per-scope aggregation bodies this module's
//! cache wraps, and `sessionize` for how checkpoint rows become sittings.

mod book;
mod compute;
mod pages;
mod sessionize;

#[cfg(test)]
mod tests;

pub use book::book_insights;
/// Per-user aggregate cache TTL. A reload after a just-finished session
/// reflects new data within this window; repeated calls inside it hit the
/// cache instead of re-running the SQL. Re-exported from `omnibus_shared` so
/// the frontend's `/stats` footer note can render the real TTL without a
/// second hardcoded copy of the number.
pub use omnibus_shared::STATS_TTL_SECS;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use omnibus_shared::{StatsRange, StatsSummary};
use sqlx::SqlitePool;

#[cfg(test)]
use compute::{
    avg_stars, books_active, books_per_month, finished_books, finished_count, genre_share,
    genre_tagged_books, listening_daily, prev_window_bounds, previous_period, rating_monthly,
    window_start, FINISHED_BOOKS_LIMIT,
};
use compute::{compute, FINISHED_EVENTS};

/// Failure space of the aggregation layer. Every metric is a SQL query, so a
/// wrapped `sqlx::Error` is the only failure — kept as an enum (rather than a
/// bare `sqlx::Error`) so no raw DB error leaks across the module boundary.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::books::BooksError> for StatsError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => StatsError::Sqlx(inner),
            // `resolve_canonical_book_uuid` (the only `BooksError`-returning
            // call this module makes) never reads overrides, so these variants
            // are unreachable here in practice — folded into a generic decode
            // error rather than panicking, mirroring `db::progress`'s same
            // mapping.
            crate::books::BooksError::OverridesJson(inner) => {
                StatsError::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
            crate::books::BooksError::Other(msg) => {
                StatsError::Sqlx(sqlx::Error::Decode(msg.into()))
            }
        }
    }
}

type Cache = Mutex<HashMap<(i64, StatsRange), (i64, StatsSummary)>>;

/// Process-wide cache shared by every request for every user — keyed by
/// `(user_id, range)` so no cached aggregate leaks between users, and read
/// through by [`user_stats_at`] before any SQL runs.
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
