//! `/api/ebooks/*` handlers.
//!
//! Cookie-gated reads that list the configured library, look up a single
//! book by uuid, and stream the raw EPUB bytes for the in-app reader.
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, scanner};
use omnibus_shared::{EbookLibrary, SortDir, SortKey, ViewFilters};
use serde::Deserialize;

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

/// Default keyset page size for the paginated `GET /api/ebooks` form (F5b
/// open question #1). A grid renders ~30–60 cards above the fold and the table
/// more; 100 covers both without an oversized first paint. An explicit
/// `?limit=` overrides it (the db layer caps it at `MAX_BOOKS_RETURNED`).
const DEFAULT_PAGE_LIMIT: i64 = 100;

/// Query params for the F5b paginated form of `GET /api/ebooks`. When none are
/// present the handler returns the full (capped) library exactly as before, so
/// existing mobile clients are unaffected; any of `sort`/`dir`/`cursor`/`limit`
/// switches it to a keyset page that also emits `X-Next-Cursor`. `sort`/`dir`
/// deserialize straight from their `snake_case`/`lowercase` wire forms
/// (`?sort=last_updated&dir=desc`). The cursor is interpreted under the
/// request's sort axis, so a `cursor` without an explicit `sort` **and** `dir`
/// is a 400 — the client repeats them on every page.
#[derive(Deserialize)]
pub(super) struct EbooksQuery {
    sort: Option<SortKey>,
    dir: Option<SortDir>,
    cursor: Option<String>,
    limit: Option<i64>,
}

/// Query parameters for `GET /api/ebooks/{uuid}/file`.
#[derive(Deserialize)]
pub(super) struct EbookFileQuery {
    file_id: Option<i64>,
}

pub(super) async fn get_ebooks(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<EbooksQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let ebook = settings.ebook_library_path;
    let audiobook = settings.audiobook_library_path;

    // Backward-compatible default: with no pagination params the response is
    // byte-identical to the pre-F5b full (capped) library, so existing mobile
    // clients keep working untouched.
    if q.sort.is_none() && q.dir.is_none() && q.cursor.is_none() && q.limit.is_none() {
        return match db::library_from_db_with_total_combined(
            &state.pool,
            ebook.as_deref(),
            audiobook.as_deref(),
        )
        .await
        {
            Ok((library, total)) => with_pagination_headers(Json(library).into_response(), total),
            Err(error) => internal("read books", error),
        };
    }

    // Paged keyset form. A cursor is decoded relative to the request's sort
    // axis, so it's only meaningful with an explicit `sort` + `dir` — enforce
    // that as a 400 rather than silently mis-positioning the page.
    if q.cursor.is_some() && (q.sort.is_none() || q.dir.is_none()) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "cursor requires sort and dir",
        )
            .into_response();
    }
    // A malformed cursor is likewise a client error (400), not a 500.
    let cursor = match q.cursor.as_deref() {
        Some(c) => match db::PageCursor::decode(c) {
            Ok(p) => Some(p),
            Err(_) => {
                return (axum::http::StatusCode::BAD_REQUEST, "malformed cursor").into_response()
            }
        },
        None => None,
    };
    let path = ebook.clone().or_else(|| audiobook.clone());
    let paths = db::collect_paths(ebook.as_deref(), audiobook.as_deref());
    let page = match db::list_books_page(
        &state.pool,
        &paths,
        q.sort.unwrap_or_default(),
        q.dir.unwrap_or_default(),
        // REST keyset is sort+cursor only; sidebar filters are a web concern.
        &ViewFilters::default(),
        cursor.as_ref(),
        q.limit.unwrap_or(DEFAULT_PAGE_LIMIT),
    )
    .await
    {
        Ok(p) => p,
        Err(error) => return internal("read books page", error),
    };
    let total = match db::count_books_for_paths(&state.pool, &paths).await {
        Ok(t) => t,
        Err(error) => return internal("count books", error),
    };

    let library = EbookLibrary {
        path,
        books: page.books,
        error: None,
        total: None,
    };
    // Unlike the full-library path, a keyset page is *not* truncated to
    // `MAX_BOOKS_RETURNED` overall (the limit is a per-page clamp), so emit the
    // true `X-Total-Count` but never `X-Total-Cap` — the page is complete.
    let mut resp = Json(library).into_response();
    if let Ok(v) = axum::http::HeaderValue::from_str(&total.to_string()) {
        resp.headers_mut().insert("X-Total-Count", v);
    }
    if let Some(next) = page.next {
        if let Ok(v) = axum::http::HeaderValue::from_str(&next.encode()) {
            resp.headers_mut().insert("X-Next-Cursor", v);
        }
    }
    resp
}

pub(super) async fn get_ebook_by_uuid(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::get_book_by_uuid(&state.pool, &uuid).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read book", error),
    }
}

/// Streams the raw EPUB bytes. Accepts optional `?file_id=N` to target
/// a specific `book_files` row for multi-EPUB books.
pub(super) async fn get_ebook_file(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(query): Query<EbookFileQuery>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let path = if let Some(file_id) = query.file_id {
        match db::book_file_path_by_id(&state.pool, id, file_id, Some("EPUB")).await {
            Ok(Some(p)) => p,
            Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
            Err(e) => return internal("book_file_path_by_id", e),
        }
    } else {
        match db::book_file_path(&state.pool, id, "EPUB").await {
            Ok(Some(p)) => p,
            Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
            Err(e) => return internal("book_file_path", e),
        }
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, "application/epub+zip"),
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Err(e) => {
            tracing::warn!(?path, error = %e, "epub file read failed");
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
    }
}

pub(super) async fn get_library(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => {
            let contents = scanner::scan_libraries(
                settings.ebook_library_path.as_deref(),
                settings.audiobook_library_path.as_deref(),
            );
            Json(contents).into_response()
        }
        Err(error) => internal("read settings", error),
    }
}

#[cfg(test)]
mod tests;
