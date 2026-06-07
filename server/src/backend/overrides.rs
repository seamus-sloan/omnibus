//! Per-book metadata override handlers (REST, mobile-facing).
//!
//! Edit-permitted users `POST` / `DELETE` overrides against a book's uuid;
//! reads happen via the standard ebook endpoints, which merge overrides
//! into the wire DTO. Mounted on the REST router in [`super::rest_router`].

// ---------------------------------------------------------------------------
// F5.1 Metadata overrides (REST — mobile client).
// ---------------------------------------------------------------------------

use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::{detect_image_format, MetadataOverrides};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Save metadata overrides for a book. Requires `can_edit` or admin.
pub(super) async fn post_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(overrides): Json<MetadataOverrides>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    if let Err(msg) = overrides.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    // Resolve uuid → id so the thumbnail/cover invalidate calls stay
    // id-keyed. Returns 404 for unknown uuids — same behavior the old
    // `get_book_uuid(id)` -> 404 had for unknown ids.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    // Merge incoming overrides with any existing ones so a second edit that
    // only touches field B doesn't wipe a prior override on field A. The
    // read-merge-write is serialized inside a single BEGIN IMMEDIATE
    // transaction in the db layer so two concurrent edits to the same book
    // can't interleave and drop each other's changes (#166).
    if let Err(e) = db::merge_metadata_overrides(&state.pool, &uuid, &overrides, user.id).await {
        return internal("merge_metadata_overrides", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Delete metadata overrides for a book, reverting to scanned values.
pub(super) async fn delete_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    if let Err(e) = db::delete_metadata_overrides(&state.pool, &uuid).await {
        return internal("delete_metadata_overrides", e);
    }
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so the axum runtime stays
    // responsive under load (#106).
    let uuid_for_blocking = uuid.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(id);
    })
    .await
    {
        return internal("spawn_blocking(delete_override_cover)", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Upload a replacement cover image for a book. Multipart form with a single
/// `cover` field containing the image bytes.
pub(super) async fn post_ebook_cover(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }

    // uuid → id for the thumb-invalidate + `get_book` calls that still
    // use the internal autoincrement key. Returns 404 for unknown uuids,
    // matching the prior `get_book_uuid(id)` → 404 behavior.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    // Extract the cover field from the multipart body.
    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "cover" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "cover must be an image",
                    )
                        .into_response();
                }
                // Reject SVG — contains executable content and can XSS when
                // opened directly in a browser tab.
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG covers are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "cover must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Validate magic bytes — don't trust Content-Type alone.
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            // Use the detected MIME so the stored extension matches
                            // actual content, not the (untrusted) client header.
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
                    Err(e) => return internal("read cover field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'cover' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    // Write the override cover to disk. `write_override_cover` is a sync
    // `std::fs` call — run it on the blocking pool so the axum runtime stays
    // responsive while we hit disk (#106). `uuid` is needed again below for
    // the overrides table update, so it's the only value we clone.
    let uuid_for_write = uuid.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        db::write_override_cover(&uuid_for_write, &mime, &bytes)
    })
    .await;
    match write_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return internal("write_override_cover", e),
        Err(e) => return internal("spawn_blocking(write_override_cover)", e),
    }

    // Mark the overrides table with has_cover_override = 1. Preserve existing
    // field overrides if any.
    let existing_overrides = match db::get_metadata_overrides(&state.pool, &uuid).await {
        Ok(Some((ov, _))) => ov,
        Ok(None) => MetadataOverrides::default(),
        Err(e) => return internal("get_metadata_overrides", e),
    };
    if let Err(e) =
        db::upsert_metadata_overrides(&state.pool, &uuid, &existing_overrides, true, user.id).await
    {
        return internal("upsert_metadata_overrides", e);
    }

    // Invalidate thumb cache so next request regenerates from new cover.
    // Also sync `std::fs` — run on the blocking pool (#106).
    if let Err(e) = tokio::task::spawn_blocking(move || db::thumbs::invalidate_thumbs(id)).await {
        return internal("spawn_blocking(invalidate_thumbs)", e);
    }

    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

#[cfg(test)]
mod tests;
