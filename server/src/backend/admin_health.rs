//! `GET /api/admin/health` handler (#952) — the first admin route in the
//! app. Aggregates index status, worker queue depth, FTS index health,
//! storage utilization, and the in-memory error ring into one
//! [`omnibus_shared::admin_health::AdminHealthReport`]. `AdminUser`-gated
//! (403 non-admin, 401 unauthenticated); web instead reaches the same
//! report via `rpc_get_admin_health` (`omnibus_frontend::rpc`), both of
//! which call `db::admin_health::build_report` — this handler exists so the
//! same data is reachable over the mobile-facing REST surface.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;

use super::{internal, AppState};
use crate::auth::AdminUser;

/// `GET /api/admin/health` — the combined server-health report. Read-only;
/// no polling built in here (see issue #955 for a future live-updating
/// variant).
pub(super) async fn get_admin_health(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::admin_health::build_report(state.pool(), state.worker()).await {
        Ok(report) => Json(report).into_response(),
        Err(error) => internal("admin health report", error),
    }
}

#[cfg(test)]
mod tests;
