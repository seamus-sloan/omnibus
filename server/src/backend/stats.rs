//! Reading-stats REST handlers for the mobile client — `GET /api/stats` and
//! `GET /api/library-size`.
//!
//! The first serves the same `db::stats` aggregate as the web
//! `/api/rpc/stats` server function, windowed by an optional `?range=` query
//! param (snake_case `StatsRange`, defaulting to the current month). The
//! second is the library-scale total behind it, unwindowed and the same for
//! every reader.

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

/// How big the library is in words, pages, and hours of audio.
///
/// Deliberately not folded into [`get_stats`]'s payload: the answer is
/// library-wide and only moves on a reindex, so carrying it there would
/// recompute and re-send it on every period switch. The auth extractor still
/// applies — the figure isn't per-user, but the library isn't public.
pub(super) async fn get_library_size(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::stats::library_size(&state.pool).await {
        Ok(size) => Json(size).into_response(),
        Err(e) => internal("get_library_size", e),
    }
}

#[cfg(test)]
mod tests;
