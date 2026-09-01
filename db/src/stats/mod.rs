//! Reading-stats aggregation over the `reading_sessions` /
//! `listening_sessions` tables plus `journal_entries` / `book_read_status`
//! for completion; no new query-time schema. See the `book`, `compute`,
//! `pages`, `patterns`, `ratings`, `streak`, and `superlatives` submodules
//! for the per-scope aggregation bodies this module's cache wraps,
//! `sessionize` for how checkpoint rows become sittings, `library` /
//! `composition` for the two aggregates here that describe the collection
//! rather than a reader, and `sessions` for the uncached per-sitting log
//! those aggregates summarize.
//!
//! # Which calendar
//!
//! Every **day boundary** here is cut on one offset per request — the asking
//! client's, via `crate::user_offset`. Days are ordinal (today, yesterday, seven
//! in a row), and a sequence measured on two calendars cannot be ordered, so
//! there is exactly one per summary.
//!
//! `patterns` is the deliberate exception, and not a second calendar: it buckets
//! by **hour of day** and weekday against the offset each session recorded at
//! capture time, so an evening read in Tokyo stays an evening after the reader
//! flies home. A shape-of-a-day distribution needs no ordering, which is why it
//! can answer a question the day boundaries cannot.

mod book;
mod builder;
mod calendar;
mod composition;
mod compute;
mod genre;
mod goals;
mod library;
mod pages;
mod patterns;
mod ratings;
// `pub(crate)` for `IDLE_GAP_SECS` alone — see its own docs.
pub(crate) mod sessionize;
mod sessions;
mod streak;
mod superlatives;

#[cfg(test)]
mod tests;

pub use book::book_insights;
pub use builder::{chart_series, ChartError};
pub use composition::{invalidate as invalidate_library_composition, library_composition};
pub use goals::{current_year, daily_goals, goal_for_year, set_daily_goal, set_goal, GoalError};
pub use library::{invalidate as invalidate_library_size, library_size};
/// Per-user aggregate cache TTL. A reload after a just-finished session
/// reflects new data within this window; repeated calls inside it hit the
/// cache instead of re-running the SQL. Re-exported from `omnibus_shared` so
/// the frontend's `/stats` footer note can render the real TTL without a
/// second hardcoded copy of the number.
pub use omnibus_shared::STATS_TTL_SECS;
pub use sessions::{session_log, SESSION_LOG_DEFAULT_LIMIT, SESSION_LOG_MAX_LIMIT};

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use omnibus_shared::{StatsRange, StatsSummary};
use sqlx::SqlitePool;

#[cfg(test)]
use compute::{
    as_of, books_active, books_per_month, finished_books, finished_count, listening_daily,
    prev_window_bounds, prev_window_from, previous_period, window_start, FINISHED_BOOKS_LIMIT,
};
use compute::{compute, FINISHED_EVENTS};
#[cfg(test)]
use genre::{genre_share, genre_tagged_books};
#[cfg(test)]
use ratings::{avg_stars, rating_monthly};

/// Failure space of the aggregation layer. Every metric is a SQL query, so a
/// wrapped `sqlx::Error` is the only failure — kept as an enum (rather than a
/// bare `sqlx::Error`) so no raw DB error leaks across the module boundary.
#[derive(Debug, thiserror::Error)]
pub enum StatsError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::user_offset::OffsetError> for StatsError {
    fn from(e: crate::user_offset::OffsetError) -> Self {
        match e {
            crate::user_offset::OffsetError::Sqlx(inner) => StatsError::Sqlx(inner),
        }
    }
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

type Cache = Mutex<HashMap<(i64, StatsRange, i64), (i64, StatsSummary)>>;

/// Process-wide cache shared by every request for every user — keyed by
/// `(user_id, range, offset_minutes)` so no cached aggregate leaks between
/// users, and read through by [`user_stats_at`] before any SQL runs.
///
/// The offset belongs in the key because it moves the answer: every day boundary
/// in a summary is cut on it, so a phone in Tokyo and a laptop in Los Angeles
/// have genuinely different summaries to be served, and omitting it would hand
/// one of them the other's days. It costs nothing in practice — a reader is in
/// one place at a time, so this stays one entry per range.
fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Aggregate one user's reading/listening stats over `range`, cached for
/// [`STATS_TTL_SECS`].
///
/// `claimed_offset_minutes` is what the asking client says its UTC offset is,
/// and every day boundary in the result is cut on it — the heatmap's columns,
/// the streak's today, both daily goals, and where the window itself begins.
/// `None` falls back to the reader's most recent session offset and then to UTC;
/// see [`crate::user_offset::resolve_offset_minutes`].
pub async fn user_stats(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
    claimed_offset_minutes: Option<i64>,
) -> Result<StatsSummary, StatsError> {
    let offset =
        crate::user_offset::resolve_offset_minutes(pool, user_id, claimed_offset_minutes).await?;
    user_stats_at(pool, user_id, range, offset, now_secs()).await
}

/// Clock-injected core of [`user_stats`], taking an already-resolved offset. A
/// cache hit fresher than the TTL returns without touching the DB; otherwise the
/// SQL runs and the result is cached under `(user_id, range, offset)` stamped at
/// `now`.
async fn user_stats_at(
    pool: &SqlitePool,
    user_id: i64,
    range: StatsRange,
    offset_minutes: i64,
    now: i64,
) -> Result<StatsSummary, StatsError> {
    if let Some(hit) = cache_get(user_id, range, offset_minutes, now) {
        return Ok(hit);
    }
    let summary = compute(pool, user_id, range, offset_minutes).await?;
    cache_put(user_id, range, offset_minutes, now, summary.clone());
    Ok(summary)
}

fn cache_get(
    user_id: i64,
    range: StatsRange,
    offset_minutes: i64,
    now: i64,
) -> Option<StatsSummary> {
    let guard = cache().lock().ok()?;
    let (at, summary) = guard.get(&(user_id, range, offset_minutes))?;
    (now.saturating_sub(*at) < STATS_TTL_SECS).then(|| summary.clone())
}

fn cache_put(
    user_id: i64,
    range: StatsRange,
    offset_minutes: i64,
    now: i64,
    summary: StatsSummary,
) {
    if let Ok(mut guard) = cache().lock() {
        // Bounded by the TTL, not a count: an entry past it can never be served
        // again, so age reclaims exactly the dead ones — where a capacity cap
        // would have to evict a live reader's summary, and a summary is not
        // small (a heatmap vec plus up to `FINISHED_BOOKS_LIMIT` books).
        guard.retain(|_, (at, _)| now.saturating_sub(*at) < STATS_TTL_SECS);
        guard.insert((user_id, range, offset_minutes), (now, summary));
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

/// Drop every cached summary for one user, across every range.
///
/// Called by a write whose result the next read must reflect immediately —
/// [`goals::set_goal`] today. Scoped to the one user so another reader's warm
/// cache isn't thrown away for a change that can't affect them.
pub fn invalidate_user(user_id: i64) {
    if let Ok(mut guard) = cache().lock() {
        guard.retain(|(uid, _, _), _| *uid != user_id);
    }
}

/// Eviction, inline rather than in the sibling `tests.rs` because it needs no
/// pool and no fixtures — the cache is a map behind an injected clock.
#[cfg(test)]
mod cache_tests {
    use super::*;

    #[test]
    fn cache_put_evicts_entries_the_ttl_has_passed() {
        // Stamped near zero so the sweep can't reach a sibling test's entry: the
        // cache is a process-wide static, and everything else in this crate is
        // stamped at `now_secs()` or at 1000.
        let stale = (9_401, StatsRange::Week, 0);
        let fresh = (9_402, StatsRange::Week, 0);
        cache_put(stale.0, stale.1, stale.2, 0, StatsSummary::default());
        cache_put(
            fresh.0,
            fresh.1,
            fresh.2,
            STATS_TTL_SECS,
            StatsSummary::default(),
        );

        // Read back at the instant it was written, where the TTL alone would
        // still have served it — so a miss here is reclamation, not expiry.
        assert!(cache_get(stale.0, stale.1, stale.2, 0).is_none());
        assert!(cache_get(fresh.0, fresh.1, fresh.2, STATS_TTL_SECS).is_some());
    }
}
