//! Reading-stats REST handler for the mobile client (`GET /api/stats`).
//!
//! Serves the same `db::stats` aggregate as the web `/api/rpc/stats` server
//! function, windowed by an optional `?range=` query param (snake_case
//! `StatsRange`, defaulting to the current month).

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db as db;
use omnibus_shared::StatsRange;
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Query shape for `GET /api/stats`. `range` is optional so a bare GET serves
/// the default window; an unknown value is a 400 from the extractor.
#[derive(Debug, Deserialize)]
pub(super) struct StatsQuery {
    #[serde(default)]
    range: StatsRange,
}

/// Fetch the authed user's stats summary over the requested range.
pub(super) async fn get_stats(
    user: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<StatsQuery>,
) -> Response {
    match db::stats::user_stats(&state.pool, user.id, query.range).await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => internal("get_stats", e),
    }
}

#[cfg(test)]
mod tests;
