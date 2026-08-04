//! Reading-stats aggregate fetch for the `/stats` page.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::{StatsRange, StatsSummary};

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
