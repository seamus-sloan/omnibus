//! Reading-stats aggregate fetch for the `/stats` page, plus the per-book
//! Insights-card counterpart for the book-detail page.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::{BookInsights, StatsRange, StatsSummary};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Fetch the current user's stats summary over `range`. Served from the
/// `db::stats` per-user cache (60s TTL), so switcher-driven refetches inside
/// the window skip the SQL. Mobile uses the analogous `GET /api/stats` REST
/// route in `server::backend::stats`.
#[post("/api/rpc/stats", pool: PoolExt, user: AuthUser)]
pub async fn rpc_stats(range: StatsRange) -> Result<StatsSummary> {
    Ok(db::stats::user_stats(&pool.0, user.id, range)
        .await
        .map_err(|e| internal_rpc_error("stats", e))?)
}

/// Fetch the current user's Started/Time-read/Sessions insights for one book
/// — the book-detail page's Insights card. `None` when the book has no
/// recorded sessions yet, driving the card's em-dash empty state. Web-only:
/// the rail section that renders this card doesn't compile on mobile, so
/// there's no REST counterpart.
#[post("/api/rpc/book-insights", pool: PoolExt, user: AuthUser)]
pub async fn rpc_book_insights(uuid: String) -> Result<Option<BookInsights>> {
    Ok(db::stats::book_insights(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| internal_rpc_error("book insights", e))?)
}
