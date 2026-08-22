//! Admin background-task history server function
//! (`/api/rpc/background-tasks`). Web-only: reads the durable
//! `background_tasks` table via `db::background_tasks::recent_tasks` and returns
//! the most recent rows, newest first. Admin-gated by the `AdminUser` extractor,
//! so a non-admin session is rejected before the table is touched.

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

/// Fetch the recent-task rows and genericize a DB failure into an internal
/// RPC error. Split out of [`rpc_get_background_tasks`] so the DB-failure
/// path is unit-testable without the `#[get]` macro's request-extraction
/// plumbing (mirrors `rpc::scan::map_scan_err`'s reason for existing).
#[cfg(feature = "server")]
async fn fetch_recent_tasks(
    pool: &sqlx::SqlitePool,
) -> Result<Vec<BackgroundTaskRecord>, ServerFnError> {
    db::background_tasks::recent_tasks(pool, RECENT_TASKS_LIMIT)
        .await
        .map_err(|e| internal_rpc_error("recent background tasks", e))
}

/// Admin-only: the most recent background-task history rows, newest first.
#[get("/api/rpc/background-tasks", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_background_tasks() -> Result<Vec<BackgroundTaskRecord>> {
    Ok(fetch_recent_tasks(&pool.0).await?)
}

// `server`-gated: exercises the DB-failure path directly against a broken
// pool, no HTTP dispatch needed. CI runs this via
// `cargo test -p omnibus-frontend --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::fetch_recent_tasks;

    #[tokio::test]
    async fn fetch_recent_tasks_genericizes_an_opaque_db_failure() {
        let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
        // Drop the table `recent_tasks` reads from so the query fails —
        // the DB-failure path this route has never had coverage for
        // (#1968).
        sqlx::query("DROP TABLE background_tasks")
            .execute(&pool)
            .await
            .unwrap();
        let err = fetch_recent_tasks(&pool).await.unwrap_err();
        assert!(err.to_string().contains("internal server error"));
    }
}
