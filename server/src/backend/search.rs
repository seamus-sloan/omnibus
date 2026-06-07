//! `GET /api/search` handler.
//!
//! Cookie-gated FTS5 search across the configured library. Returns a
//! capped result vec plus the full hit count via `X-Total-Count` /
//! `X-Total-Cap` headers so clients can detect truncation without parsing
//! the body. Mounted on a sub-router that carries its own rate limit.

use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use serde::Deserialize;

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
}

pub(super) async fn get_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        // Match the `/api/ebooks` contract: even an empty result attaches
        // `X-Total-Count: 0` so clients can rely on the header always
        // being present.
        return with_pagination_headers(
            Json(omnibus_shared::EbookLibrary::default()).into_response(),
            0,
        );
    };
    // Issue #241: one FTS5 pass yields both the (capped) vec and the *full*
    // hit count via a scalar COUNT over the materialized matches CTE, replacing
    // the prior search_books + count_search_books double pass.
    let (books, total) = match db::search_books_with_total(&state.pool, &path, &params.q).await {
        Ok(pair) => pair,
        Err(error) => return internal("search books", error),
    };
    // The full hit count rides the `X-Total-Count` / `X-Total-Cap` headers so
    // clients can detect truncation without changing the JSON body shape.
    let body = Json(omnibus_shared::EbookLibrary {
        path: Some(path),
        books,
        error: None,
        total: None,
    })
    .into_response();
    with_pagination_headers(body, total)
}

pub(super) async fn get_search_palette(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(omnibus_shared::PaletteResults::default()).into_response();
    };
    match db::search_palette(&state.pool, &path, &params.q).await {
        Ok(results) => Json(results).into_response(),
        Err(error) => internal("search palette", error),
    }
}

#[cfg(test)]
mod tests;
