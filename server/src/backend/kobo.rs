//! Native Kobo wireless sync (`/kobo/<TOKEN>/v1/*`).
//!
//! Mounted OUTSIDE the `/api/*` auth gate (see `main.rs`); every route
//! authenticates via the path token ([`KoboAuthUser`]). Slice A implements the
//! core surface — `library/sync` (streamed, no item cap), `library/<uuid>/
//! metadata`, `download`, `library/<uuid>/state` (PUT), `library/tags`, and the
//! cover image route. The per-device delta cursor (#923), per-shelf opt-in
//! (#924), analytics, and the annotation channel (#927) layer on later.

use axum::{
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, put},
    Extension, Json, Router,
};
use futures_util::stream;
use omnibus_db::{
    self as db,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::{ReadStatus, SetReadStatus};

use super::{internal, serve_download, AppState};

mod dto;
mod extractor;
#[cfg(test)]
mod tests;

use extractor::KoboAuthUser;

/// Wall-clock budget for an inline KEPUB conversion on a Kobo download before
/// falling back to plain EPUB. Mirrors the USB sideload path's budget.
const KEPUB_CONVERT_BUDGET: std::time::Duration = std::time::Duration::from_secs(25);

/// Build the wireless Kobo router. `Extension(pool)` is layered here so the
/// router is self-contained for integration tests; the live server adds the
/// same one at the top (harmless overlap, mirroring `rest_router`).
pub fn kobo_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/kobo/{token}/v1/library/sync", get(library_sync))
        .route(
            "/kobo/{token}/v1/library/{uuid}/metadata",
            get(library_metadata),
        )
        .route("/kobo/{token}/v1/library/{uuid}/state", put(put_state))
        .route("/kobo/{token}/v1/library/tags", get(library_tags))
        .route("/kobo/{token}/v1/download/{uuid}", get(download))
        .route(
            "/kobo/{token}/v1/books/{uuid}/thumbnail/{w}/{h}/{quality}/{greyscale}/image.jpg",
            get(image),
        )
        .with_state(state)
        .layer(Extension(pool))
}

/// `GET library/sync` — enumerate every opted-in book as a `NewEntitlement`,
/// streamed as a JSON array via [`Body::from_stream`]. Deliberately imposes no
/// item cap (unlike Calibre-Web's `SYNC_ITEM_LIMIT=100`). Slice A returns the
/// whole library on every call; the per-device delta cursor is #923.
async fn library_sync(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let books = match db::kobo::sync_books(state.pool()).await {
        Ok(b) => b,
        Err(e) => return internal("kobo sync_books", e),
    };
    let base = origin_from_headers(&headers);

    // One JSON chunk per entitlement, framed as an array. No cap.
    let mut chunks: Vec<Result<Bytes, std::io::Error>> = Vec::with_capacity(books.len() + 2);
    chunks.push(Ok(Bytes::from_static(b"[")));
    for (i, book) in books.iter().enumerate() {
        let item = dto::new_entitlement(&base, &auth.token, book, 0);
        let json = match serde_json::to_vec(&item) {
            Ok(v) => v,
            Err(e) => return internal("kobo serialize entitlement", e),
        };
        let mut piece = if i > 0 { vec![b','] } else { Vec::new() };
        piece.extend_from_slice(&json);
        chunks.push(Ok(Bytes::from(piece)));
    }
    chunks.push(Ok(Bytes::from_static(b"]")));

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            // Opaque per-device cursor; the device echoes it back but we never
            // parse it (the real per-device snapshot cursor is #923).
            (
                header::HeaderName::from_static("x-kobo-synctoken"),
                "slice-a",
            ),
        ],
        Body::from_stream(stream::iter(chunks)),
    )
        .into_response()
}

/// `GET library/<uuid>/metadata` — the single-book metadata array Kobo fetches
/// before downloading.
async fn library_metadata(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    match db::kobo::book_for_sync(state.pool(), &uuid).await {
        Ok(Some(book)) => {
            let base = origin_from_headers(&headers);
            Json(vec![dto::book_metadata(&base, &auth.token, &book, 0)]).into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("kobo book_for_sync", e),
    }
}

/// `GET download/<uuid>` — serve the book as KEPUB (converting via the worker,
/// cached), falling back to plain EPUB when kepubify is absent, conversion
/// fails, or it exceeds [`KEPUB_CONVERT_BUDGET`]. Streamed with range support.
async fn download(
    _auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
    req: Request,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(state.pool(), &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("kobo resolve_book_id_by_uuid", e),
    };
    let path = if kepub_ready(&state, id).await {
        db::kepub_path(id)
    } else {
        match db::book_file_path(state.pool(), id, "EPUB").await {
            Ok(Some(p)) => p,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return internal("kobo book_file_path", e),
        }
    };
    serve_download(req, &path, "application/epub+zip").await
}

/// `PUT library/<uuid>/state` — persist the device's reading state. Slice A
/// routes `StatusInfo` into the existing `book_read_status` table; the
/// `CurrentBookmark` position is a `KoboSpan`, not a CFI, so it is **not**
/// written to `reading_progress` yet (the span↔CFI decision is #925).
async fn put_state(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
    Json(body): Json<dto::StateRequest>,
) -> Response {
    for entry in &body.reading_states {
        if let Some(info) = &entry.status_info {
            let update = SetReadStatus {
                book_uuid: uuid.clone(),
                status: map_status(&info.status),
            };
            match db::read_status::set_read_status(state.pool(), auth.user.id, &update).await {
                Ok(_) => {}
                // A state push for a book the server never indexed is not fatal
                // to the sync — log and keep the success envelope.
                Err(db::read_status::ReadStatusError::BookNotFound) => {
                    tracing::warn!(%uuid, "kobo state PUT for unknown book");
                }
                Err(db::read_status::ReadStatusError::Sqlx(e)) => {
                    return internal("kobo set_read_status", e);
                }
            }
        }
        if entry.current_bookmark.is_some() {
            tracing::debug!(%uuid, "kobo position received; KoboSpan→CFI deferred to #925");
        }
    }
    Json(dto::StateResponse::success(uuid)).into_response()
}

/// `GET library/tags` — device-side collections. Slice A returns an empty set;
/// shelves-as-collections is #924.
async fn library_tags(_auth: KoboAuthUser) -> Response {
    Json(serde_json::json!([])).into_response()
}

/// `GET books/<uuid>/thumbnail/.../image.jpg` — serve the book cover. The
/// requested dimensions are advisory; slice A serves the stored cover as-is.
async fn image(
    _auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid, _w, _h, _quality, _greyscale)): Path<(
        String,
        String,
        u32,
        u32,
        u32,
        String,
    )>,
) -> Response {
    let id = match db::resolve_book_id_by_uuid(state.pool(), &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("kobo image resolve", e),
    };
    match db::get_cover(state.pool(), id).await {
        Ok(Some((mime, bytes))) => ([(header::CONTENT_TYPE, mime)], bytes).into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("kobo get_cover", e),
    }
}

/// Enqueue the (idempotent, cache-backed) KEPUB conversion and wait up to
/// [`KEPUB_CONVERT_BUDGET`]. `true` when the cache is ready to serve.
async fn kepub_ready(state: &AppState, book_id: i64) -> bool {
    let task_id = state.worker().post(Task::KepubConvert { book_id });
    match tokio::time::timeout(
        KEPUB_CONVERT_BUDGET,
        state.worker().await_completion(task_id),
    )
    .await
    {
        Ok(TaskOutcome::Ok(_)) => true,
        Ok(TaskOutcome::Err(msg)) => {
            tracing::warn!(book_id, error = %msg, "kobo kepub conversion failed; serving plain epub");
            false
        }
        Err(_elapsed) => {
            tracing::warn!(
                book_id,
                "kobo kepub conversion exceeded budget; serving plain epub"
            );
            false
        }
    }
}

/// Map a Kobo `StatusInfo.Status` token to the internal read status. Unknown
/// tokens (and `ReadyToRead`) fall back to `Unread`.
fn map_status(kobo: &str) -> ReadStatus {
    match kobo {
        "Finished" => ReadStatus::Finished,
        "Reading" => ReadStatus::Reading,
        _ => ReadStatus::Unread,
    }
}

/// Reconstruct the request origin (`scheme://host`) from headers, so the
/// device-facing download URLs are absolute. Honors `X-Forwarded-Proto` when a
/// reverse proxy sets it; defaults to `http`.
fn origin_from_headers(headers: &HeaderMap) -> String {
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}
