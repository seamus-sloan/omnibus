//! Star-rating REST handlers for the mobile client (`/api/ratings*`): set, get,
//! and clear a half-star rating (`0.5..=5.0`) per `(user, book)`. Writes are
//! last-write-wins, with the range and half-step validated at the boundary so
//! an invalid value returns 400. Web clients use the `/api/rpc/ratings*` server
//! functions in `omnibus_frontend::rpc`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, ratings::RatingError};
use omnibus_shared::RatingUpdate;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Persist a new rating. Last-write-wins on `(user, book)`; returns the
/// server-authoritative record so the caller can sync forward. 400 on an
/// out-of-range star count, 404 for a book the server has never indexed.
pub(super) async fn post_rating(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<RatingUpdate>,
) -> Response {
    if let Err(msg) = update.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::ratings::set_rating(&state.pool, user.id, &update).await {
        Ok(rec) => Json(rec).into_response(),
        Err(RatingError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(RatingError::Sqlx(e)) => internal("set_rating", e),
    }
}

/// Fetch the current rating for `(user, uuid)`. Returns `200 { … }` with an
/// `Option<RatingRecord>` body (`null` when the user has not rated the book).
pub(super) async fn get_rating(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::ratings::get_rating(&state.pool, user.id, &uuid).await {
        Ok(rec) => Json(rec).into_response(),
        Err(e) => internal("get_rating", e),
    }
}

/// Fetch every other user's attributed rating for a book, newest first.
pub(super) async fn get_other_ratings(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::ratings::list_other_ratings(&state.pool, user.id, &uuid).await {
        Ok(ratings) => Json(ratings).into_response(),
        Err(e) => internal("get_other_ratings", e),
    }
}

/// Clear (un-rate) the current user's rating for a book. Idempotent — clearing
/// an absent rating still returns 204.
pub(super) async fn delete_rating(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::ratings::delete_rating(&state.pool, user.id, &uuid).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("delete_rating", e),
    }
}

#[cfg(test)]
mod tests;
