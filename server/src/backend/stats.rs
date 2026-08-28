//! Reading-stats REST handlers for the mobile client. `GET /api/stats` serves
//! the same `db::stats` aggregate as the web `/api/rpc/stats` server function,
//! windowed by an optional `?range=` param (snake_case `StatsRange`, default
//! month). `PUT /api/stats/goal` sets the annual goal; `GET /api/library-size`
//! and `GET /api/library-composition` describe the collection itself — its
//! scale and its mix — the same for every reader.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, stats::GoalError};
use omnibus_shared::{ReadingGoalUpdate, StatsRange};
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

/// Set, change, or clear the caller's annual reading goal.
///
/// `year` defaults to the server's current calendar year and `kind` to
/// `books`, so the usual body is `{"target": 24}`; a `null` / absent `target`
/// clears the goal. Returns the resulting goal (`null` once cleared) with its
/// progress already recomputed, and `db::stats::set_goal` drops this user's
/// cached summaries so the next read is not the pre-save one.
///
/// This is account configuration under rule 08 and must never be queued by an
/// offline outbox — clients call it directly and disable the control offline.
pub(super) async fn put_stats_goal(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<ReadingGoalUpdate>,
) -> Response {
    match db::stats::set_goal(&state.pool, user.id, &update).await {
        Ok(goal) => Json(goal).into_response(),
        Err(GoalError::Sqlx(e)) => internal("set_reading_goal", e),
        // The remaining variants are all boundary checks on the payload, and
        // each `#[error]` message names the bound it failed.
        Err(
            e @ (GoalError::UnsupportedKind(_)
            | GoalError::InvalidTarget(_)
            | GoalError::InvalidYear(_)),
        ) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// What the library is made of — format, language, publisher, publication
/// decade, and genre.
///
/// Its own route rather than a field on [`get_stats`]'s payload for the same
/// reason [`get_library_size`] is: the answer is library-wide and only moves
/// on a reindex, so carrying it there would recompute and re-send it on every
/// period switch. The auth extractor still applies — the figures aren't
/// per-user, but the library isn't public.
pub(super) async fn get_library_composition(
    _user: AuthUser,
    State(state): State<AppState>,
) -> Response {
    match db::stats::library_composition(&state.pool).await {
        Ok(composition) => Json(composition).into_response(),
        Err(e) => internal("get_library_composition", e),
    }
}

#[cfg(test)]
mod tests;
