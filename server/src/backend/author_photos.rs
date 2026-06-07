//! Author-photo serve + admin override handlers.
//!
//! Cookie-gated `GET` returns the cached author photo bytes (404 on miss
//! kicks off background Open Library resolution); admin-only `PUT` /
//! `DELETE` accepts an uploaded photo or a remote URL via the SSRF-guarded
//! `fetch_remote_image` helper.

// ---------------------------------------------------------------------------
// F1.11 Author profile photos.
// ---------------------------------------------------------------------------

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, worker::Task};
use omnibus_shared::detect_image_format;
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

/// Serve a cached author profile photo. Returns 404 when no photo is cached
/// (including `'letter'` negative-cache markers) — the frontend keeps the
/// letter avatar in that case. On a miss, enqueues a background resolution
/// task so a subsequent page view can render the resolved photo.
pub(super) async fn get_author_photo(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author_photo(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Match `/api/covers/:id` cache semantics.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => {
            // Only queue resolution if no row exists yet — a `letter` marker
            // is a sticky miss until an admin DELETEs it. `author_photo_status`
            // returns the row regardless of source.
            if let Ok(None) = db::author_photo_status(&state.pool, id).await {
                state
                    .worker
                    .post(Task::ResolveAuthorPhoto { author_id: id });
            }
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => internal("read author photo", error),
    }
}

/// Admin upload of a manual author profile photo. Multipart with one
/// `photo` field. Mirrors the `/api/ebooks/{id}/cover` validation
/// pipeline: 10 MiB cap, magic-byte sniff, SVG rejection.
pub(super) async fn put_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    // Confirm the author exists before reading multipart so a malformed
    // upload to a missing id fails fast with 404 (not 400).
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "photo" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "photo must be an image",
                    )
                        .into_response();
                }
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG photos are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "photo must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            Some(mime) => break (mime, b),
                            None => {
                                return (
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => return internal("read photo field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'photo' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        None,
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

/// Admin: drop the cached photo so the next page view re-queues resolution.
pub(super) async fn delete_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::delete_author_photo(&state.pool, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("delete_author_photo", e),
    }
}

/// Admin: synchronously run the Open Library cascade for an author. Clears
/// any sticky `letter` negative-cache marker so the resolver re-queries Open
/// Library, then awaits the resolver inline and returns
/// `{ "resolved": bool }`. `resolved=false` means Open Library had nothing
/// (a `letter` marker is now in place to skip future autoresolution).
///
/// Manual uploads are treated as overrides: a `source = 'manual'` row is
/// preserved and Scan returns `resolved=true` without touching the row,
/// so admins can't accidentally wipe a manual upload by clicking the
/// button.
pub(super) async fn post_author_photo_scan(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    // Verify the author exists first so a typo on the id gets a 404 instead
    // of a successful no-op scan.
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }
    // Manual uploads win — don't delete or overwrite. Treat scan as a no-op
    // and report resolved=true so the UI keeps the existing photo.
    match db::author_photo_status(&state.pool, id).await {
        Ok(Some((db::AuthorPhotoSource::Manual, _))) => {
            return Json(omnibus_shared::AuthorPhotoScanResult { resolved: true }).into_response();
        }
        Ok(_) => {}
        Err(e) => return internal("author_photo_status (pre-scan)", e),
    }
    if let Err(e) = db::delete_author_photo(&state.pool, id).await {
        return internal("delete_author_photo (pre-scan)", e);
    }
    if let Err(e) = db::author_photos::resolve(&state.pool, id).await {
        return internal("author_photos::resolve", e);
    }
    let resolved = match db::get_author_photo(&state.pool, id).await {
        Ok(opt) => opt.is_some(),
        Err(e) => return internal("get_author_photo (post-scan)", e),
    };
    Json(omnibus_shared::AuthorPhotoScanResult { resolved }).into_response()
}

/// Admin: bulk re-resolve all author photos via the background worker.
/// Returns 202 Accepted immediately; progress is polled via
/// `/api/rpc/worker_status`.
pub(super) async fn post_refetch_author_photos(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Response {
    state.worker.post(Task::RefetchAuthorPhotos);
    axum::http::StatusCode::ACCEPTED.into_response()
}

/// JSON body for [`put_author_photo_url`]. Kept inline because the shape is
/// trivial and not shared with any other call site (the RPC server function
/// passes the URL as a positional arg).
#[derive(Debug, Deserialize)]
pub(super) struct AuthorPhotoUrlBody {
    url: String,
}

/// Admin: persist an author photo by URL. Server-side fetches the URL,
/// validates content-type/size/magic-bytes, and stores it as a `manual`
/// row — the same source as a multipart upload, so it wins over Open
/// Library resolution and survives a "Scan for picture" click.
pub(super) async fn put_author_photo_url(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AuthorPhotoUrlBody>,
) -> Response {
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let url = body.url.trim();
    if url.is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, "url is required").into_response();
    }

    let (advertised_mime, bytes) =
        match db::author_photos::fetch_remote_image_with(url, state.remote_image_config()).await {
            Ok(pair) => pair,
            Err(db::author_photos::FetchRemoteImageError::Http(e)) => {
                return internal("fetch remote image", e);
            }
            Err(e) => {
                // All other variants are validation errors (bad scheme, blocked
                // SSRF target, non-image content-type, too-large, …) — map to
                // 400 with the user-facing message from `#[error]`.
                return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response();
            }
        };

    // Magic-byte sniff — don't trust the remote Content-Type header.
    let mime = match detect_image_format(&bytes) {
        Some(m) => m,
        None => {
            tracing::warn!(
                advertised_mime,
                "remote URL returned image content-type but bytes are not a recognized image"
            );
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "file at URL does not appear to be a valid image",
            )
                .into_response();
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        Some(url),
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo (url)", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests;
