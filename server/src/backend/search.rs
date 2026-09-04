//! `GET /api/search` family: metadata search, book-content search, and the
//! command palette.
//!
//! Cookie-gated FTS5 search across the configured libraries. The metadata
//! search returns a capped result vec plus the full hit count via
//! `X-Total-Count` / `X-Total-Cap` headers; all three routes are mounted on
//! a sub-router that carries its own per-IP rate limit.

use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::{search_query_too_long, SpoilerFilter};
#[cfg(test)]
use omnibus_shared::SEARCH_QUERY_MAX_LEN as MAX_SEARCH_QUERY_LEN;
use serde::Deserialize;

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
}

/// `GET /api/search/content` params. Everything past `q` is optional, so a
/// caller that only knows the old shape still works.
#[derive(Deserialize)]
pub(super) struct ContentSearchQuery {
    q: String,
    /// Scope to one book. Repeatable via the comma-separated `book_uuids`.
    book_uuid: Option<String>,
    /// Comma-separated book uuids, for scoping to a series.
    book_uuids: Option<String>,
    /// Hit ceiling; server-clamped.
    limit: Option<i64>,
    /// What to do about hits past the reader's own position.
    #[serde(default)]
    spoiler_filter: SpoilerFilter,
}

impl ContentSearchQuery {
    /// The uuids this request scopes to, from either spelling.
    fn scope_uuids(&self) -> Vec<String> {
        let mut uuids: Vec<String> = Vec::new();
        if let Some(one) = self.book_uuid.as_deref() {
            uuids.push(one.trim().to_string());
        }
        if let Some(many) = self.book_uuids.as_deref() {
            uuids.extend(
                many.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(str::to_string),
            );
        }
        uuids.retain(|u| !u.is_empty());
        uuids.sort();
        uuids.dedup();
        uuids
    }
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

/// `GET /api/search/content` — full-text search over indexed EPUB chapter
/// text (`book_content_fts`). Hits cite `(book_uuid, spine_index)` plus the
/// chapter title and an FTS5 `snippet()` excerpt; an unconfigured library or
/// an empty query yields an empty hit list rather than an error.
///
/// `book_uuid` / `book_uuids` scope the search, `limit` caps it, and
/// `spoiler_filter` places each hit against the caller's own position —
/// annotating by default, excluding on request. An empty result carries a
/// `hint` when the query *form* is the likely cause.
pub(super) async fn get_search_content(
    user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<ContentSearchQuery>,
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
        return Json(omnibus_shared::ContentSearchResults::default()).into_response();
    }
    let scope = db::ContentSearchScope {
        book_uuids: params.scope_uuids(),
        limit: params.limit,
    };
    let mut hits = match db::search_content_for_paths(&state.pool, &paths, &params.q, &scope).await
    {
        Ok(hits) => hits,
        Err(error) => return internal("search content", error),
    };
    let withheld =
        match db::annotate_spoilers(&state.pool, user.id, &mut hits, params.spoiler_filter).await {
            Ok(withheld) => withheld,
            Err(error) => return internal("annotate spoilers", error),
        };
    // Only worth explaining when there is nothing to show: a hint alongside
    // results would be noise, and the per-term counts cost a query each.
    let hint = if hits.is_empty() && withheld.unwrap_or(0) == 0 {
        match db::explain_empty_content_search(&state.pool, &paths, &params.q, &scope).await {
            Ok(hint) => hint,
            Err(error) => return internal("explain content search", error),
        }
    } else {
        None
    };
    Json(omnibus_shared::ContentSearchResults {
        hits,
        withheld_ahead: withheld,
        hint,
    })
    .into_response()
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
