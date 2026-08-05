//! `GET /api/genres` handler.
//!
//! Cookie-gated read returning the genre cloud (genre name + book count) for
//! the configured library. Mirrors `/api/tags`; backs the genre chip-editor's
//! autocomplete pool on the native clients.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::AuthUser;

pub(super) async fn get_genres(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_genre_cloud(&state.pool).await {
        Ok(genres) => Json(genres).into_response(),
        Err(error) => internal("read genres", error),
    }
}

#[cfg(test)]
mod tests;
