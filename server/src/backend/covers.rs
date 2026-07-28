//! `/api/covers/*` and `/api/thumbs/*` handlers.
//!
//! Session-gated reads that resolve a book uuid through the db layer and
//! stream the cached cover or WebP thumbnail bytes back to the client.
//! Authenticated via [`MediaAuthUser`] (cookie / bearer / `?token=` query),
//! the last so the mobile WebView's `<img>` fetches can carry the session.
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap},
    response::{IntoResponse, Response},
};
use omnibus_db::{self as db};

use super::validator::{if_none_match_hits, not_modified, MEDIA_VARY, REVALIDATE};
use super::{internal, AppState};
use crate::auth::MediaAuthUser;

pub(super) async fn get_cover(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    headers: HeaderMap,
) -> Response {
    tracing::debug!(uuid, "cover: request received");
    // Resolve uuid → id so the existing id-keyed `db::get_cover` (which
    // reads cover bytes from `<covers_dir>/<uuid>.<ext>` by way of the
    // books row) stays unchanged. The route surface is uuid-keyed so
    // bookmarked URLs survive reindexes; the storage layer keeps using
    // the autoincrement id internally for join performance.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            tracing::warn!(uuid, "cover: book not found (404)");
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => {
            let etag = content_etag(&bytes);
            if if_none_match_hits(&headers, &etag) {
                tracing::debug!(uuid, book_id = id, "cover: not modified (304)");
                return not_modified(&etag, REVALIDATE, MEDIA_VARY);
            }
            tracing::debug!(
                uuid,
                book_id = id,
                mime,
                bytes = bytes.len(),
                "cover: serving"
            );
            (
                [
                    (header::CONTENT_TYPE, mime.as_str()),
                    // A book editor cover replace (#1086) writes new bytes
                    // under the *same* uuid-keyed URL, so a stale
                    // `max-age`-only cache would keep serving the old image
                    // for up to a day on the next reload/revisit — the
                    // browser never even asks. `no-cache` forces a
                    // conditional GET on every load; the `ETag` below makes
                    // that revalidation a cheap 304 whenever the cover
                    // hasn't actually changed. `private` + `Vary` (see
                    // [`MEDIA_VARY`]) keep a shared proxy from serving one
                    // user's covers to an unauthenticated (or differently
                    // bearer-authenticated) request on the same URL now
                    // that the endpoint is gated.
                    (header::CACHE_CONTROL, REVALIDATE),
                    (header::ETAG, etag.as_str()),
                    (header::VARY, MEDIA_VARY),
                    // Prevent browsers from MIME-sniffing a cover into an
                    // executable type (e.g. an SVG disguised as JPEG).
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => {
            tracing::warn!(uuid, book_id = id, "cover: no cover image on record (404)");
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => internal("read cover", error),
    }
}

/// Cheap content-derived `ETag` for a served image. Not cryptographic, so
/// a hash collision between two genuinely different byte sequences is
/// possible — that's a *correctness* failure, not a benign inefficiency:
/// [`if_none_match_hits`] would treat a stale `If-None-Match` as current,
/// the handler would return a bodyless 304, and the client would keep
/// showing the old image indefinitely instead of fetching the real
/// update. A stronger digest would shrink that (already astronomically
/// small) probability further, but isn't applied here since collisions
/// self-heal on the next byte change anyway.
fn content_etag(bytes: &[u8]) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("\"{:016x}\"", hasher.finish())
}

pub(super) async fn get_thumb(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path((uuid, size_str)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    tracing::debug!(uuid, size = %size_str, "thumb: request received");
    let size: db::ThumbSize = match size_str.parse() {
        Ok(s) => s,
        Err(_) => {
            tracing::warn!(uuid, size = %size_str, "thumb: invalid size parameter (400)");
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid size; use sm, md, or lg",
            )
                .into_response();
        }
    };
    // uuid → id translation: see the comment in `get_cover` for why we
    // resolve at the edge rather than rewriting the thumbnail pipeline
    // to be uuid-keyed end-to-end.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => {
            tracing::warn!(uuid, "thumb: book not found (404)");
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    let last_modified_epoch = match db::get_last_modified_epoch(&state.pool, id).await {
        Ok(Some(ts)) => ts,
        Ok(None) => {
            tracing::warn!(
                uuid,
                book_id = id,
                "thumb: book has no last_modified_epoch (404)"
            );
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
        Err(e) => return internal("read last_modified_epoch", e),
    };

    // Cache hit: thumb exists and is fresh. Use async I/O here so a hot
    // `srcset` grid doesn't pin tokio worker threads on the synchronous read.
    let thumb_path = db::thumb_path_for(id, size);
    if !db::thumbs::is_stale_async(id, size, last_modified_epoch).await {
        if let Ok(bytes) = tokio::fs::read(&thumb_path).await {
            // Fire-and-forget mtime bump so `evict_if_over_cap` treats this
            // as recently-used (LRU) instead of evicting frequently-viewed
            // thumbs just because they're old. Detached via `tokio::spawn`
            // (never adds latency to this response) but still awaits the
            // `spawn_blocking` JoinHandle and distinguishes panic from
            // cancellation, matching the worker's convention
            // (`handle_generate_thumbs` in db/src/worker/handlers.rs)
            // instead of silently dropping it.
            tokio::spawn(async move {
                if let Err(join_err) =
                    tokio::task::spawn_blocking(move || db::thumbs::touch_thumb(id, size)).await
                {
                    let kind = if join_err.is_panic() {
                        "panicked"
                    } else {
                        "was cancelled"
                    };
                    tracing::warn!(error = %join_err, book_id = id, "thumbs: touch_thumb {kind}");
                }
            });
            let etag = content_etag(&bytes);
            if if_none_match_hits(&headers, &etag) {
                tracing::debug!(uuid, book_id = id, ?size, "thumb: not modified (304)");
                return not_modified(&etag, REVALIDATE, MEDIA_VARY);
            }
            tracing::debug!(
                uuid,
                book_id = id,
                ?size,
                bytes = bytes.len(),
                "thumb: cache hit"
            );
            // Same rationale as `get_cover` (#1086): the server regenerates
            // this WebP as soon as the underlying cover changes
            // (`invalidate_thumbs`), but a `max-age`-only Cache-Control never
            // lets the browser notice — it keeps serving its own day-old
            // copy from the identical URL. `no-cache` + `ETag` make every
            // reload check back, cheaply, via a 304 when nothing changed.
            return (
                [
                    (header::CONTENT_TYPE, "image/webp"),
                    (header::CACHE_CONTROL, REVALIDATE),
                    (header::ETAG, etag.as_str()),
                    (header::VARY, MEDIA_VARY),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // Cache miss or stale: fetch the original cover first so we only queue
    // generation when there's actually something to thumbnail. Queuing for
    // a coverless book just produces a guaranteed `no cover for book …`
    // worker error on every request, polluting the log.
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => {
            tracing::debug!(
                uuid,
                book_id = id,
                ?size,
                "thumb: cache miss — queuing generation, serving original cover"
            );
            state.worker.post(db::worker::Task::GenerateThumbs {
                book_id: id,
                last_modified_epoch,
            });
            (
                [
                    (header::CONTENT_TYPE, mime.as_str()),
                    // Short TTL: browser will re-fetch after ~5 s when the WebP is ready.
                    (header::CACHE_CONTROL, "private, max-age=5"),
                    (header::VARY, MEDIA_VARY),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => {
            tracing::debug!(
                uuid,
                book_id = id,
                "thumb: cache miss and no cover image — returning 202"
            );
            axum::http::StatusCode::ACCEPTED.into_response()
        }
        Err(e) => internal("cover fetch for thumb", e),
    }
}

#[cfg(test)]
mod tests;
