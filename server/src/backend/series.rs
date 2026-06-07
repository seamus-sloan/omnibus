//! `/api/series/*` handlers.
//!
//! Cookie-gated reads returning the series browse-all index and
//! per-series detail (books in series order, primary author, accent).
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// `/series` index. Returns every series across both ebook and audiobook
/// libraries with a book count, primary author, and optional accent.
pub(super) async fn get_series(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    let paths = db::collect_paths(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    );
    match db::list_series(&state.pool, &paths).await {
        Ok(series) => Json(series).into_response(),
        Err(e) => internal("list series", e),
    }
}

pub(super) async fn get_series_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_series(&state.pool, id).await {
        Ok(Some(series)) => Json(series).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read series", error),
    }
}

#[cfg(test)]
mod tests;
