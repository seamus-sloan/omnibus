//! `GET /api/admin/health` handler — the first admin route in the app.
//! Aggregates index status, worker queue, FTS health, storage, and the
//! in-memory error ring into one `AdminHealthReport`. `AdminUser`-gated;
//! the mobile-facing REST twin of `rpc_get_admin_health`, both calling
//! `db::admin_health::build_report` so the two surfaces can't drift.

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
