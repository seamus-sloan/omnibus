//! Admin background-task history server function (`/api/rpc/background-tasks`).
//!
//! Web-only surface: reads the durable `background_tasks` table (migration
//! `0070`, issue #941) via `db::background_tasks::recent_tasks` and returns
//! the most recent rows, newest first. Admin-gated by the `AdminUser`
//! extractor, so a non-admin session is rejected before the table is
//! touched — mirrors `rpc_get_last_errors`'s shape exactly.

use dioxus::fullstack::get;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::BackgroundTaskRecord;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, PoolExt};

/// Cap on rows returned by [`rpc_get_background_tasks`] — enough history for
/// an operator to spot a pattern without an unbounded response.
#[cfg(feature = "server")]
const RECENT_TASKS_LIMIT: i64 = 100;

/// Admin-only: the most recent background-task history rows, newest first.
#[get("/api/rpc/background-tasks", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_background_tasks() -> Result<Vec<BackgroundTaskRecord>> {
    Ok(
        db::background_tasks::recent_tasks(&pool.0, RECENT_TASKS_LIMIT)
            .await
            .map_err(|e| internal_rpc_error("recent background tasks", e))?,
    )
}
