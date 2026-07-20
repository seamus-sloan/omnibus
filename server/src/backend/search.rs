//! `GET /api/search` handler.
//!
//! Cookie-gated FTS5 search across the configured libraries. Returns a capped
//! result vec plus the full hit count via `X-Total-Count` / `X-Total-Cap`
//! headers; mounted on a sub-router that carries its own rate limit.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::search_query_too_long;
#[cfg(test)]
use omnibus_shared::SEARCH_QUERY_MAX_LEN as MAX_SEARCH_QUERY_LEN;
use serde::Deserialize;

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
}

/// Reject an over-length query with 400 so oversized input is never forwarded
/// into the search db calls. Returns `Some(response)` when `q` exceeds
/// `omnibus_shared::SEARCH_QUERY_MAX_LEN` — the same cap the RPC search
/// entrypoints enforce via [`search_query_too_long`].
fn reject_if_over_length(q: &str) -> Option<Response> {
    search_query_too_long(q).then(|| (StatusCode::BAD_REQUEST, "query too long").into_response())
}

pub(super) async fn get_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    if let Some(rejection) = reject_if_over_length(&params.q) {
        return rejection;
    }
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let ebook = settings.ebook_library_path;
    let audiobook = settings.audiobook_library_path;
    let paths = db::collect_paths(ebook.as_deref(), audiobook.as_deref());
    if paths.is_empty() {
        // Match the `/api/ebooks` contract: even an empty result attaches
        // `X-Total-Count: 0` so clients can rely on the header always
        // being present.
        return with_pagination_headers(
            Json(omnibus_shared::EbookLibrary::default()).into_response(),
            0,
        );
    }
    let path = paths[0].to_string();
    // Issue #241: one FTS5 pass yields both the (capped) vec and the *full*
    // hit count via a scalar COUNT over the materialized matches CTE, replacing
    // the prior search_books + count_search_books double pass.
    let (books, total) =
        match db::search_books_for_paths_with_total(&state.pool, &paths, &params.q).await {
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
    if let Some(rejection) = reject_if_over_length(&params.q) {
        return rejection;
    }
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let paths = db::collect_paths(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    );
    if paths.is_empty() {
        return Json(omnibus_shared::PaletteResults::default()).into_response();
    }
    match db::search_palette_for_paths(&state.pool, &paths, &params.q).await {
        Ok(results) => Json(results).into_response(),
        Err(error) => internal("search palette", error),
    }
}

#[cfg(test)]
mod tests;
