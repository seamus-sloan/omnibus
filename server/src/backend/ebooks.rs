//! `/api/ebooks/*` handlers.
//!
//! Cookie-gated reads that list the configured library, look up a single
//! book by uuid, and stream the raw EPUB bytes for the in-app reader.
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    self as db, scanner,
    worker::{Task, TaskOutcome},
};
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
    let path = ebook.as_ref().or(audiobook.as_ref()).cloned();
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

/// Resolve the on-disk EPUB path for `uuid`, honouring an optional
/// `?file_id=N` (multi-EPUB books). Returns the error `Response`
/// (404 / 500) already formed on the failure paths so callers can `?`-style
/// early-return with `Err`.
async fn resolve_epub_path(
    state: &AppState,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<std::path::PathBuf, Response> {
    let id = match db::resolve_book_id_by_uuid(&state.pool, uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return Err(axum::http::StatusCode::NOT_FOUND.into_response()),
        Err(e) => return Err(internal("resolve_book_id_by_uuid", e)),
    };
    // Carry the context alongside the chosen query so a 500 points at the
    // call that actually failed rather than always blaming `book_file_path`.
    let (resolved, ctx) = if let Some(file_id) = file_id {
        (
            db::book_file_path_by_id(&state.pool, id, file_id, Some("EPUB")).await,
            "book_file_path_by_id",
        )
    } else {
        (
            db::book_file_path(&state.pool, id, "EPUB").await,
            "book_file_path",
        )
    };
    match resolved {
        Ok(Some(p)) => Ok(p),
        Ok(None) => Err(axum::http::StatusCode::NOT_FOUND.into_response()),
        Err(e) => Err(internal(ctx, e)),
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
    let path = match resolve_epub_path(&state, &uuid, query.file_id).await {
        Ok(p) => p,
        Err(resp) => return resp,
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

/// Wall-clock budget for an inline KEPUB conversion before we give up and
/// serve plain EPUB. Kept under the global 30 s request timeout so a slow
/// book downloads *something* rather than 408-ing; the worker keeps running
/// and warms the cache for next time.
const KEPUB_CONVERT_BUDGET: std::time::Duration = std::time::Duration::from_secs(25);

/// Convert `book_id`'s EPUB to KEPUB (via the worker, cached) and download it
/// for USB sideload onto a Kobo. Falls back to the plain EPUB when kepubify is
/// absent, conversion fails, or it exceeds [`KEPUB_CONVERT_BUDGET`] — a Kobo
/// reads plain EPUB too, just with slower page turns. The download filename is
/// the canonical `book_uuid` so a future USB annotation import can map the
/// device's `ContentID` back to the book.
pub(super) async fn get_ebook_kepub(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    // Canonical uuid (merged-uuid aware) is the stable filename stem.
    let canonical = match db::resolve_canonical_book_uuid(&state.pool, &uuid).await {
        Ok(Some(u)) => u,
        Ok(None) => uuid.clone(),
        Err(e) => return internal("resolve_canonical_book_uuid", e),
    };

    if kepub_conversion_succeeded(&state, id).await {
        let path = db::kepub_path(id);
        match tokio::fs::read(&path).await {
            Ok(bytes) => return download_response(bytes, &format!("{canonical}.kepub.epub")),
            Err(e) => {
                tracing::warn!(book_id = id, ?path, error = %e, "kepub cache read failed; serving plain epub");
            }
        }
    }

    // Fallback: the plain EPUB.
    let path = match db::book_file_path(&state.pool, id, "EPUB").await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("book_file_path", e),
    };
    match tokio::fs::read(&path).await {
        Ok(bytes) => download_response(bytes, &format!("{canonical}.epub")),
        Err(e) => {
            tracing::warn!(book_id = id, ?path, error = %e, "epub file read failed");
            StatusCode::NOT_FOUND.into_response()
        }
    }
}

/// Enqueue the (idempotent, cache-backed) KEPUB conversion and wait up to
/// [`KEPUB_CONVERT_BUDGET`]. `true` when the cache is ready to serve.
async fn kepub_conversion_succeeded(state: &AppState, book_id: i64) -> bool {
    let task_id = state.worker.post(Task::KepubConvert { book_id });
    match tokio::time::timeout(KEPUB_CONVERT_BUDGET, state.worker.await_completion(task_id)).await {
        Ok(TaskOutcome::Ok) => true,
        Ok(TaskOutcome::Err(msg)) => {
            tracing::warn!(book_id, error = %msg, "kepub conversion failed; serving plain epub");
            false
        }
        Err(_elapsed) => {
            tracing::warn!(
                book_id,
                "kepub conversion exceeded budget; serving plain epub"
            );
            false
        }
    }
}

/// Build an `attachment` download response for EPUB/KEPUB bytes with the given
/// filename. Both formats are `application/epub+zip`; the `.kepub.epub`
/// extension is what tells a Kobo to render it as a KEPUB.
fn download_response(bytes: Vec<u8>, filename: &str) -> Response {
    let disposition = format!("attachment; filename=\"{filename}\"");
    let mut resp = bytes.into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/epub+zip"),
    );
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&disposition)
            .unwrap_or_else(|_| HeaderValue::from_static("attachment")),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, max-age=0"),
    );
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    resp
}

/// Serves the raw EPUB as a browser download (`Content-Disposition:
/// attachment`). Same path resolution as [`get_ebook_file`]; only the
/// disposition differs, so the in-app reader keeps streaming inline via
/// `/file` while the export menu drives a real save-to-disk via `/download`.
pub(super) async fn get_ebook_download(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(query): Query<EbookFileQuery>,
    req: Request,
) -> Response {
    let path = match resolve_epub_path(&state, &uuid, query.file_id).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    super::serve_download(req, &path, "application/epub+zip").await
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
