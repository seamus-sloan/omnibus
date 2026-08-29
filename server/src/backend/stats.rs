//! Reading-stats REST handlers for the mobile client: the `db::stats`
//! aggregate (`GET /api/stats`, windowed by an optional snake_case `?range=`),
//! the same data un-aggregated as the caller's own keyset-paginated session
//! log (`GET /api/stats/sessions`), the annual goal (`PUT /api/stats/goal`)
//! and the daily ones (`PUT /api/stats/goal/daily`),
//! and the collection's own scale and mix (`GET /api/library-size`,
//! `GET /api/library-composition` — the same for every reader).

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, stats::GoalError};
use omnibus_shared::{DailyGoalUpdate, ReadingGoalUpdate, SessionCursor, StatsRange};
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
        // `InvalidDailyTarget` is unreachable here — `set_goal` bounds against
        // the annual maximum — but it is a 400 wherever it does arise, so it is
        // answered rather than left to widen this match into a catch-all.
        Err(
            e @ (GoalError::UnsupportedKind(_)
            | GoalError::InvalidTarget(_)
            | GoalError::InvalidDailyTarget { .. }
            | GoalError::InvalidYear(_)),
        ) => (StatusCode::BAD_REQUEST, e.to_string()).into_response(),
    }
}

/// Set, change, or clear one of the caller's daily reading goals.
///
/// The body names its `kind` — `pages` or `minutes` — and an absent `target`
/// clears that kind, leaving the other and the annual goal alone. Returns
/// **both** daily goals with today's progress recomputed, so a client can
/// redraw the whole band from one response rather than following a save with a
/// read that could observe a different day.
///
/// Its own route rather than a widened `PUT /api/stats/goal`: that route
/// answers with an `Option<ReadingGoal>` that shipped iOS builds already
/// decode, and growing its response to carry a daily goal too would break every
/// one of them.
///
/// Account configuration under rule 08, like the annual goal — never queued by
/// an offline outbox.
pub(super) async fn put_stats_daily_goal(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<DailyGoalUpdate>,
) -> Response {
    match db::stats::set_daily_goal(&state.pool, user.id, &update).await {
        Ok(goals) => Json(goals).into_response(),
        Err(GoalError::Sqlx(e)) => internal("set_daily_reading_goal", e),
        // As above: every other variant is a boundary check the payload failed.
        Err(
            e @ (GoalError::UnsupportedKind(_)
            | GoalError::InvalidTarget(_)
            | GoalError::InvalidDailyTarget { .. }
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

/// Query shape for `GET /api/stats/sessions`. Every field is optional: a bare
/// GET serves the newest page of the caller's whole log.
#[derive(Debug, Deserialize)]
pub(super) struct SessionLogQuery {
    /// Scope to one book. Resolved through the merge ledger, so a link into a
    /// book that was later merged away still finds its sittings.
    book: Option<String>,
    /// Page size, clamped by `db::stats` into
    /// `1..=SESSION_LOG_MAX_LIMIT`.
    limit: Option<i64>,
    /// The previous page's `next_before`, echoed back verbatim.
    before: Option<String>,
}

/// Fetch a page of the authed user's session log, newest sitting first.
///
/// Scoped to `user.id` from the token — there is no user parameter, so no
/// caller can ask for someone else's log. A `before` that isn't a cursor this
/// endpoint issued is a 400 rather than a silent rewind to page one, which
/// would loop a paging client forever.
pub(super) async fn get_session_log(
    user: AuthUser,
    State(state): State<AppState>,
    Query(query): Query<SessionLogQuery>,
) -> Response {
    let before = match query.before.as_deref() {
        Some(raw) => match SessionCursor::parse(raw) {
            Some(cursor) => Some(cursor),
            None => {
                return (axum::http::StatusCode::BAD_REQUEST, "invalid before cursor")
                    .into_response()
            }
        },
        None => None,
    };
    let limit = query.limit.unwrap_or(db::stats::SESSION_LOG_DEFAULT_LIMIT);
    match db::stats::session_log(
        &state.pool,
        user.id,
        query.book.as_deref(),
        before.as_ref(),
        limit,
    )
    .await
    {
        Ok(page) => Json(page).into_response(),
        Err(e) => internal("get_session_log", e),
    }
}
