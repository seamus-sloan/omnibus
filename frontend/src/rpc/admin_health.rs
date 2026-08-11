//! Admin server-health report server function. Web-only, admin-gated by
//! the `AdminUser` extractor; composes the same `AdminHealthReport` the
//! REST twin serves. `POST` (not `GET`) because the body needs
//! `WorkerExt` — see `rpc_worker_status`'s doc comment in `settings.rs`
//! for why a `WorkerExt`-carrying `#[get]` route silently 404s.
use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::admin_health::AdminHealthReport;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, PoolExt, WorkerExt};

/// Admin-only: the full `/admin/health` report in one request. Called both
/// on the page's first load and on its 5-second live-update poll (#955) —
/// this endpoint itself stays a plain point-in-time fetch; the interval
/// lives client-side in `pages::admin_health`.
#[post("/api/rpc/admin-health", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_get_admin_health() -> Result<AdminHealthReport> {
    Ok(db::admin_health::build_report(&pool.0, &worker.0)
        .await
        .map_err(|e| internal_rpc_error("admin health report", e))?)
}
