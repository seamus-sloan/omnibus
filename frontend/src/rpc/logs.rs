//! Admin log-viewer server function (`/api/rpc/logs`).
//!
//! Web-only surface: reads the on-disk JSON logs via `db::logs` and returns
//! one filtered, newest-first page. Admin-gated by the `AdminUser` extractor,
//! so a non-admin session is rejected before any file is touched.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::logs::{LogPage, LogQuery};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, PoolExt};

/// Admin-only: paginated, filtered read over the on-disk JSON logs. `POST`
/// (not `GET`) because it carries a [`LogQuery`] body and to stay on the same
/// server-fn code path the other body-bearing routes use. `_pool` is unused —
/// the `AdminUser` extractor resolves the session itself — but kept so every
/// route lists the pool extension identically.
#[post("/api/rpc/logs", _pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_logs(query: LogQuery) -> Result<LogPage> {
    Ok(db::logs::read_logs(&db::logs::log_dir(), &query)
        .await
        .map_err(|e| internal_rpc_error("read logs", e))?)
}
