//! `GET /api/tags` handler.
//!
//! Cookie-gated read returning the tag cloud (tag name + weight) for the
//! configured library. Powers the tag-cloud discovery page on mobile.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::AuthUser;

pub(super) async fn get_tags(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_tag_cloud(&state.pool).await {
        Ok(tags) => Json(tags).into_response(),
        Err(error) => internal("read tags", error),
    }
}

#[cfg(test)]
mod tests;
