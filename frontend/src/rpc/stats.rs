//! Reading-stats aggregate fetch for the `/stats` page, the library-scale
//! totals beside it, and the per-book Insights-card counterpart for the
//! book-detail page.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
#[cfg(feature = "server")]
use omnibus_shared::SessionCursor;
use omnibus_shared::{
    BookInsights, ChartResult, ChartSpec, DailyGoalUpdate, DailyGoals, LibraryComposition,
    LibrarySize, ReadingGoal, ReadingGoalUpdate, SessionLogPage, StatsRange, StatsSummary,
};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Fetch the current user's stats summary over `range`. Served from the
/// `db::stats` per-user cache (60s TTL, keyed on the offset too), so
/// switcher-driven refetches inside the window skip the SQL. Mobile uses the
/// analogous `GET /api/stats` REST route in `server::backend::stats`.
///
/// `utc_offset_minutes` is where the calling device is, and every day boundary
/// in the answer is cut on it (rule 10). `None` — an older client, or one with
/// no browser to ask — falls back to the reader's most recent session offset
/// and then to UTC.
#[post("/api/rpc/stats", pool: PoolExt, user: AuthUser)]
pub async fn rpc_stats(range: StatsRange, utc_offset_minutes: Option<i64>) -> Result<StatsSummary> {
    Ok(
        db::stats::user_stats(&pool.0, user.id, range, utc_offset_minutes)
            .await
            .map_err(|e| internal_rpc_error("stats", e))?,
    )
}

/// Fetch the current user's Started/Time-read/Sessions insights for one book
/// — the book-detail page's Insights card. `None` when the book has no
/// recorded sessions yet, driving the card's em-dash empty state. Web-only:
/// the rail section that renders this card doesn't compile on mobile, so
/// there's no REST counterpart.
#[post("/api/rpc/book-insights", pool: PoolExt, user: AuthUser)]
pub async fn rpc_book_insights(
    uuid: String,
    utc_offset_minutes: Option<i64>,
) -> Result<Option<BookInsights>> {
    Ok(
        db::stats::book_insights(&pool.0, user.id, &uuid, utc_offset_minutes)
            .await
            .map_err(|e| internal_rpc_error("book insights", e))?,
    )
}

/// Fetch how big the library is in words, pages, and hours of audio.
///
/// Its own call rather than a field on [`rpc_stats`]'s payload: the answer is
/// library-wide and only moves on a reindex, so hanging it off the per-user
/// summary would recompute and re-send it on every period switch. Mobile uses
/// `GET /api/library-size`.
#[post("/api/rpc/library-size", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_library_size() -> Result<LibrarySize> {
    Ok(db::stats::library_size(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("library size", e))?)
}

/// Set, change, or clear the current user's annual reading goal, returning the
/// resulting goal (`None` once cleared) with its progress recomputed.
///
/// Account configuration under rule 08 — never queued by the offline outbox.
/// `db::stats::set_goal` drops this user's cached summaries, so the refetch
/// that follows a save reads the new target rather than the cached one.
#[post("/api/rpc/stats-goal", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_reading_goal(
    update: ReadingGoalUpdate,
    utc_offset_minutes: Option<i64>,
) -> Result<Option<ReadingGoal>> {
    Ok(
        db::stats::set_goal(&pool.0, user.id, &update, utc_offset_minutes)
            .await
            .map_err(|e| internal_rpc_error("set reading goal", e))?,
    )
}

/// Set, change, or clear one of the current user's daily reading goals,
/// returning **both** afterwards with today's progress recomputed.
///
/// Both rather than just the kind written, so the band redraws from one
/// response — the two are independent, and a follow-up read to fetch the other
/// could land on a different day. The offset decides *which* day both kinds
/// are measured over, so a save that omitted it could answer against a
/// different one than the read it replaces. Account configuration under rule
/// 08, so never queued by the offline outbox.
#[post("/api/rpc/stats-goal-daily", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_daily_goal(
    update: DailyGoalUpdate,
    utc_offset_minutes: Option<i64>,
) -> Result<DailyGoals> {
    Ok(
        db::stats::set_daily_goal(&pool.0, user.id, &update, utc_offset_minutes)
            .await
            .map_err(|e| internal_rpc_error("set daily reading goal", e))?,
    )
}

/// Fetch what the library is made of — its format, language, publisher,
/// decade, and genre mix.
///
/// Its own call rather than a field on [`rpc_stats`]'s payload, for the same
/// reason [`rpc_library_size`] is: the answer is library-wide and only moves
/// on a reindex, so hanging it off the per-user summary would recompute and
/// re-send it on every period switch. Mobile uses `GET /api/library-composition`.
#[post("/api/rpc/library-composition", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_library_composition() -> Result<LibraryComposition> {
    Ok(db::stats::library_composition(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("library composition", e))?)
}

/// Fetch one page of the current user's session log, newest sitting first.
/// `book` scopes it to a single book (the book-detail Stats stop); `before` is
/// the previous page's `next_before`, echoed back verbatim. Mobile uses the
/// analogous `GET /api/stats/sessions` REST route.
///
/// A `before` that doesn't parse is an **error**, matching the REST route's
/// 400. Returning an empty page instead would end the reader's log early with
/// no cursor and no message — silently truncating rather than reporting.
#[post("/api/rpc/stats/sessions", pool: PoolExt, user: AuthUser)]
pub async fn rpc_session_log(
    book: Option<String>,
    before: Option<String>,
) -> Result<SessionLogPage> {
    let cursor = match before.as_deref() {
        Some(raw) => match SessionCursor::parse(raw) {
            Some(cursor) => Some(cursor),
            None => return Err(ServerFnError::new("invalid before cursor").into()),
        },
        None => None,
    };
    Ok(db::stats::session_log(
        &pool.0,
        user.id,
        book.as_deref(),
        cursor.as_ref(),
        db::stats::SESSION_LOG_DEFAULT_LIMIT,
    )
    .await
    .map_err(|e| internal_rpc_error("session log", e))?)
}

/// Run a chart-builder spec and return its aligned series.
///
/// Uncached, unlike [`rpc_stats`]: the spec space is open, so there is no
/// small key to cache on. Web-only — the builder page doesn't compile on
/// mobile — so there is no REST counterpart in `server::backend`.
///
/// A rejected spec surfaces its own message, because it describes the
/// reader's own selection and is the thing they can act on; a DB failure is
/// logged and generalized like every other handler here. The spec is
/// re-validated server-side inside `db::stats::chart_series` — the builder
/// UI's guards are a convenience, not the contract.
#[post("/api/rpc/chart-series", pool: PoolExt, user: AuthUser)]
pub async fn rpc_chart_series(
    spec: ChartSpec,
    utc_offset_minutes: Option<i64>,
) -> Result<ChartResult> {
    Ok(
        db::stats::chart_series(&pool.0, user.id, &spec, utc_offset_minutes)
            .await
            .map_err(|e| match e {
                db::stats::ChartError::Spec(inner) => {
                    dioxus::prelude::ServerFnError::new(inner.to_string())
                }
                db::stats::ChartError::Stats(inner) => internal_rpc_error("chart series", inner),
            })?,
    )
}
