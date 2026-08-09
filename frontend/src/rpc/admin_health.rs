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

/// Admin-only: the full `/admin/health` report in one request. No polling —
/// the page fetches this once on load (see issue #955 for a future
/// live-updating variant).
#[post("/api/rpc/admin-health", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_get_admin_health() -> Result<AdminHealthReport> {
    Ok(db::admin_health::build_report(&pool.0, &worker.0)
        .await
        .map_err(|e| internal_rpc_error("admin health report", e))?)
}
