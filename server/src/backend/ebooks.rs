//! `/api/ebooks/*` handlers.
//!
//! Session-gated reads that list the configured library, look up a single
//! book by uuid, and stream the raw EPUB bytes for the in-app reader.
//! Mounted on the mobile REST router in [`super::rest_router`]. The
//! `/file` stream additionally accepts a `?token=` query param
//! ([`MediaAuthUser`]) so epub.js can fetch the book from the mobile WebView,
//! which carries neither a cookie nor an `Authorization` header.

use axum::{
    extract::{Path, Query, Request, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    self as db, scanner,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::{EbookLibrary, SortDir, SortKey, ViewFilters};
use serde::Deserialize;

use super::conditional::{self, MEDIA_CACHE_CONTROL, MEDIA_VARY};
use super::{internal, with_pagination_headers, AppState};
use crate::auth::{AuthUser, MediaAuthUser};

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
    /// Comma-separated `book_files.format` filter values (lowercase wire
    /// form, e.g. `?formats=m4b,m4a,mp3`). The mobile Sort & filter sheet's
    /// format chips; the other web sidebar facets stay RPC-only.
    formats: Option<String>,
}

/// Split the `?formats=` wire value into filter entries, dropping empties.
fn parse_formats(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_lowercase)
        .collect()
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
    if q.sort.is_none()
        && q.dir.is_none()
        && q.cursor.is_none()
        && q.limit.is_none()
        && q.formats.is_none()
    {
        return respond_full_library(&state, ebook.as_deref(), audiobook.as_deref()).await;
    }

    respond_keyset_page(&state, &q, ebook.as_deref(), audiobook.as_deref()).await
}

/// Full (capped) combined library, with `X-Total-Count` / `X-Total-Cap`
/// headers — the pre-F5b response shape for clients that send no pagination.
async fn respond_full_library(
    state: &AppState,
    ebook: Option<&str>,
    audiobook: Option<&str>,
) -> Response {
    match db::library_from_db_with_total_combined(&state.pool, ebook, audiobook).await {
        Ok((library, total)) => with_pagination_headers(Json(library).into_response(), total),
        Err(error) => internal("read books", error),
    }
}

/// Keyset-paginated page for an explicit `sort`/`dir`/`cursor`/`limit`. A
/// cursor is decoded relative to the request's sort axis, so a cursor without
/// an explicit `sort` **and** `dir`, or a malformed cursor, is a 400 rather
/// than a silently mis-positioned page or a 500.
async fn respond_keyset_page(
    state: &AppState,
    q: &EbooksQuery,
    ebook: Option<&str>,
    audiobook: Option<&str>,
) -> Response {
    if q.cursor.is_some() && (q.sort.is_none() || q.dir.is_none()) {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            "cursor requires sort and dir",
        )
            .into_response();
    }
    let cursor = match q.cursor.as_deref() {
        Some(c) => match db::PageCursor::decode(c) {
            Ok(p) => Some(p),
            Err(_) => {
                return (axum::http::StatusCode::BAD_REQUEST, "malformed cursor").into_response()
            }
        },
        None => None,
    };
    let path = ebook.or(audiobook).map(str::to_string);
    let paths = db::collect_paths(ebook, audiobook);
    // Format chips are the only REST-exposed facet (the mobile sheet); the
    // remaining sidebar facets stay a web/RPC concern.
    let filters = ViewFilters {
        formats: q.formats.as_deref().map(parse_formats).unwrap_or_default(),
        ..ViewFilters::default()
    };
    let page = match db::list_books_page(
        &state.pool,
        &paths,
        q.sort.unwrap_or_default(),
        q.dir.unwrap_or_default(),
        &filters,
        cursor.as_ref(),
        q.limit.unwrap_or(DEFAULT_PAGE_LIMIT),
    )
    .await
    {
        Ok(p) => p,
        Err(error) => return internal("read books page", error),
    };
    // Filtered total, so the client's "Show N books" reflects the active
    // chips; identical to the unfiltered count when no filter is set.
    let total = match db::count_books_page(&state.pool, &paths, &filters).await {
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
        Ok(Some(mut book)) => {
            attach_comic_page_count(&state, &mut book).await;
            Json(book).into_response()
        }
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read book", error),
    }
}

/// Fill `page_count` on a detail payload when the book carries a CBZ, by
/// listing the archive's pages — the count the pager's slider and progress
/// mapping key on. Best-effort: a missing or malformed archive logs and
/// leaves `None`, because a broken file must not take the whole detail read
/// down with it.
async fn attach_comic_page_count(state: &AppState, book: &mut omnibus_shared::EbookMetadata) {
    if !book.formats.iter().any(|f| f.eq_ignore_ascii_case("cbz")) {
        return;
    }
    let path = match db::book_file_path(&state.pool, book.id, "CBZ").await {
        Ok(Some(p)) => p,
        Ok(None) => return,
        Err(e) => {
            tracing::warn!(book_id = book.id, error = %e, "comic page count: file path lookup failed");
            return;
        }
    };
    match tokio::task::spawn_blocking(move || db::comic::list_pages(&path)).await {
        Ok(Ok(pages)) => book.page_count = Some(pages.len() as i64),
        Ok(Err(e)) => {
            tracing::warn!(book_id = book.id, error = %e, "comic page count: archive unreadable")
        }
        Err(e) => {
            tracing::warn!(book_id = book.id, error = %e, "comic page count: task join failed")
        }
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

/// Wire mime for a served CBZ archive. The registered comic-book zip type,
/// which downloaders and shelf apps recognize where a bare
/// `application/zip` would not say what the bytes are.
const CBZ_MIME: &str = "application/vnd.comicbook+zip";

/// Resolve the file `/file` streams for `uuid`: the EPUB when the book has
/// one, else its CBZ archive. The fallback is what lets a comic-only book be
/// taken offline whole — the per-page endpoint answers reading, not
/// downloading — while a dual-format book keeps serving the EPUB, matching
/// the pager's rule that the EPUB stays the primary read. An explicit
/// `?file_id=` stays EPUB-scoped: multi-file selection exists for
/// multi-EPUB books and must not silently resolve to an archive.
async fn resolve_readable_path(
    state: &AppState,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<(std::path::PathBuf, &'static str), Response> {
    match resolve_epub_path(state, uuid, file_id).await {
        Ok(path) => Ok((path, "application/epub+zip")),
        Err(resp) if file_id.is_none() && resp.status() == StatusCode::NOT_FOUND => {
            let id = match db::resolve_book_id_by_uuid(&state.pool, uuid).await {
                Ok(Some(id)) => id,
                Ok(None) => return Err(StatusCode::NOT_FOUND.into_response()),
                Err(e) => return Err(internal("resolve_book_id_by_uuid", e)),
            };
            match db::book_file_path(&state.pool, id, "CBZ").await {
                Ok(Some(path)) => Ok((path, CBZ_MIME)),
                Ok(None) => Err(StatusCode::NOT_FOUND.into_response()),
                Err(e) => Err(internal("book_file_path", e)),
            }
        }
        Err(resp) => Err(resp),
    }
}

/// Streams the raw EPUB bytes — or, for a comic-only book, the CBZ archive
/// (the whole-file download the offline clients pull). Accepts optional
/// `?file_id=N` to target a specific `book_files` row for multi-EPUB books.
///
/// Gated by [`MediaAuthUser`] rather than [`AuthUser`]: epub.js fetches this
/// URL from inside the mobile WebView, which can carry neither a session
/// cookie nor a bearer header, so the session token rides a `?token=` query
/// param. The web reader's same-origin cookie fetch keeps working unchanged.
pub(super) async fn get_ebook_file(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(query): Query<EbookFileQuery>,
    req: Request,
) -> Response {
    let mut resp = read_ebook_file(&state, &uuid, query.file_id, req).await;
    // epub.js reads this stream over a cross-origin XHR from inside the mobile
    // WebView (unlike the CORS-exempt `<img>`/`<audio>` media the app uses
    // elsewhere), so the ACAO must ride *every* response — including the 404 /
    // 500 error paths — or the WebView can't read the outcome. The URL query
    // token is the only credential, so a wildcard never widens real access.
    resp.headers_mut().insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    resp
}

/// Resolve the book's readable file (EPUB, else CBZ) and stream its bytes,
/// or report the resolution failure as a 404 / 500. CORS is layered on by
/// the caller so it covers every arm uniformly.
///
/// Goes through the shared `serve_file` path rather than buffering the whole
/// book, which is what gives the endpoint `Range` support — without which
/// the `If-Range` guarantee would have nothing to guard. No
/// `Content-Disposition`: this one is read inline by the reader, while
/// `/download` is the save-to-disk sibling.
async fn read_ebook_file(
    state: &AppState,
    uuid: &str,
    file_id: Option<i64>,
    req: Request,
) -> Response {
    let (path, mime) = match resolve_readable_path(state, uuid, file_id).await {
        Ok(resolved) => resolved,
        Err(resp) => return resp,
    };
    super::serve_file(req, &path, mime, None).await
}

/// Serve one page image out of the book's CBZ archive, extracted
/// server-side so clients never download or unzip whole archives. `page`
/// is the 0-based index into the natural-sort page order — the same order
/// `page_count` on the detail payload counts.
///
/// Gated by [`MediaAuthUser`] like the other media routes: the pager loads
/// pages via plain `<img>` fetches, which from the mobile WebView carry
/// neither a cookie nor a bearer header, so the session token rides
/// `?token=`.
///
/// The single entry is decompressed into memory, so the validator is a
/// content hash — the covers/thumbs shape, with the 304 carrying the same
/// `Cache-Control`/`Vary` as a 200. 404 covers an unknown uuid, a book
/// without a CBZ file, and an out-of-range index; an unreadable archive
/// surfaces as a 500.
pub(super) async fn get_ebook_page(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path((uuid, page)): Path<(String, usize)>,
    headers: HeaderMap,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    let path = match db::book_file_path(&state.pool, id, "CBZ").await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("book_file_path", e),
    };
    let read = tokio::task::spawn_blocking(move || db::comic::read_page(&path, page)).await;
    let (mime, bytes) = match read {
        Ok(Ok(Some(entry))) => entry,
        Ok(Ok(None)) => return StatusCode::NOT_FOUND.into_response(),
        Ok(Err(e)) => return internal("read comic page", e),
        Err(e) => return internal("read comic page", e),
    };
    let etag = conditional::content_etag(&bytes);
    let inm_hit = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| conditional::if_none_match_hits(v, &etag));
    if inm_hit {
        return conditional::not_modified(&etag, MEDIA_CACHE_CONTROL, MEDIA_VARY);
    }
    (
        [
            (header::CONTENT_TYPE, mime),
            (header::CACHE_CONTROL, MEDIA_CACHE_CONTROL),
            (header::ETAG, etag.as_str()),
            (header::VARY, MEDIA_VARY),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        bytes,
    )
        .into_response()
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

    // Fallback: the plain EPUB — still override-baked so a Kobo without
    // kepubify support (or after a conversion failure) shows the user's edits.
    let source = match db::book_file_path(&state.pool, id, "EPUB").await {
        Ok(Some(p)) => p,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("book_file_path", e),
    };
    let path = rewritten_or_source(&state, id, source).await;
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
        Ok(TaskOutcome::Ok(_)) => true,
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
    let source = match resolve_epub_path(&state, &uuid, query.file_id).await {
        Ok(p) => p,
        Err(resp) => return resp,
    };
    // A saved download must carry the user's metadata/cover edits (F5.8 #1372),
    // so serve the override-baked EPUB when the book has any; otherwise the
    // source verbatim.
    let path = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => rewritten_or_source(&state, id, source).await,
        _ => source,
    };
    // The validator is taken from whichever file is actually sent. For an
    // override-having book that is the export-cache copy, whose own stat
    // moves when `books.last_modified` invalidates it — so it tracks the
    // bytes on the wire rather than the source they came from.
    super::serve_download(req, &path, "application/epub+zip").await
}

/// The override-baked export EPUB for `book_id` when the book has edits, else
/// `source` unchanged. A rewrite failure logs and falls back to `source` — an
/// export must always download *something*. `pub(super)` so the Kobo plain-
/// EPUB download fallback (`backend::kobo::download`) shares it rather than
/// serving the raw on-disk file when overrides exist.
pub(super) async fn rewritten_or_source(
    state: &AppState,
    book_id: i64,
    source: std::path::PathBuf,
) -> std::path::PathBuf {
    match db::rewritten_epub_path(&state.pool, book_id, &source).await {
        Ok(Some(rewritten)) => rewritten,
        Ok(None) => source,
        Err(e) => {
            tracing::warn!(book_id, error = %e, "epub override-rewrite failed; serving source");
            source
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

/// Current validators for a batch of downloaded files.
///
/// A device holding N downloads needs to know whether any of their library
/// files moved. Asking per book meant N full metadata fetches on a timer —
/// a request-per-download polling loop that is a real data and battery cost
/// on a phone and a real load cost on the server. This answers all of them
/// in one small request, and carries no metadata: just the validator per
/// (book, format, file) the caller names.
///
/// A file the caller asks about that is gone, or whose row the scanner has
/// not stat'd, comes back with no `etag` — which every client reads as
/// "can't tell", never as "unchanged".
pub(super) async fn post_download_validators(
    _user: AuthUser,
    State(state): State<AppState>,
    Json(request): Json<omnibus_shared::DownloadValidatorRequest>,
) -> Response {
    if request.files.len() > omnibus_shared::MAX_VALIDATOR_QUERY {
        return (
            StatusCode::UNPROCESSABLE_ENTITY,
            "too many files in one validator request",
        )
            .into_response();
    }
    match db::download_validators(&state.pool, &request.files).await {
        Ok(files) => Json(omnibus_shared::DownloadValidatorResponse { files }).into_response(),
        Err(error) => internal("download validators", error),
    }
}
