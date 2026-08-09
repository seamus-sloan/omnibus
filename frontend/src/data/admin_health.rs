//! Admin server-health report data access (#952).
//!
//! Web/SSR-only: `/admin/health` is not a mobile surface, so there is no
//! `reqwest`/REST counterpart — the whole module is gated off the mobile
//! build and calls the `rpc_get_admin_health` server function directly,
//! same shape as `errors`/`logs`.
#![cfg(not(feature = "mobile"))]

use omnibus_shared::admin_health::AdminHealthReport;

use super::{note_server_fn_err, DataError};

/// Fetch the combined `/admin/health` report (index status, worker queue,
/// FTS health, storage, last errors). Admin-gated server-side by the
/// `rpc_get_admin_health` `AdminUser` extractor.
pub async fn get_admin_health() -> Result<AdminHealthReport, DataError> {
    crate::rpc::rpc_get_admin_health()
        .await
        .map_err(note_server_fn_err)
}
