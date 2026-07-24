//! Read/unread-state REST handlers for the mobile client (`/api/read-status*`).
//!
//! Set/get the per-`(user, book)` read state (`unread` / `reading` /
//! `finished`). Writes are last-write-wins; the uuid is validated at the
//! boundary so a blank value returns 400 and an unindexed book returns 404. Web
//! clients use the `/api/rpc/read-status/*` server functions in
//! `omnibus_frontend::rpc`.

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, read_status::ReadStatusError};
use omnibus_shared::SetReadStatus;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Persist a read state. Last-write-wins on `(user, book)`; returns the
/// server-authoritative record so the caller can sync forward. 400 on a blank
/// uuid, 404 for a book the server has never indexed.
pub(super) async fn put_read_status(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<SetReadStatus>,
) -> Response {
    if let Err(msg) = update.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::read_status::set_read_status(&state.pool, user.id, &update).await {
        Ok(rec) => Json(rec).into_response(),
        Err(ReadStatusError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ReadStatusError::Sqlx(e)) => internal("set_read_status", e),
    }
}

/// Fetch the current read state for `(user, uuid)`. Returns `200 { … }` with an
/// `Option<ReadStatusRecord>` body (`null` when the book has no row — the
/// client treats that as `unread`).
pub(super) async fn get_read_status(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::read_status::get_read_status(&state.pool, user.id, &uuid).await {
        Ok(rec) => Json(rec).into_response(),
        Err(e) => internal("get_read_status", e),
    }
}

#[cfg(test)]
mod tests;
