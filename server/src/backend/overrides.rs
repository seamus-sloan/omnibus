//! Per-book metadata override handlers (REST, mobile-facing).
//!
//! Edit-permitted users `POST` / `DELETE` overrides against a book's uuid;
//! reads happen via the standard ebook endpoints, which merge overrides
//! into the wire DTO. Mounted on the REST router in [`super::rest_router`].

use axum::{
    body::Bytes,
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::MetadataOverrides;

use super::image_upload::extract_validated_image;
use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

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
    let (mime, bytes) = match extract_validated_image(&mut multipart, "cover").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };

    if let Err(response) = persist_cover(&state, &uuid, user.id, mime, bytes).await {
        return response;
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

/// Revert an overridden cover back to the scanned original, preserving any
/// other field overrides on the book. Requires `can_edit` or admin. Mirrors
/// `delete_ebook_overrides` but only clears the cover half of the override
/// row — see [`db::clear_cover_override`].
pub(super) async fn delete_ebook_cover(
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
    if let Err(e) = db::clear_cover_override(&state.pool, &uuid, user.id).await {
        return internal("clear_cover_override", e);
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

/// Write the new override cover to disk and mark `has_cover_override = 1`,
/// preserving any existing field overrides. Returns the error `Response`
/// already formed on failure so the caller can early-return.
///
/// The prior overrides row is read BEFORE touching disk: `write_override_cover`
/// deletes any existing `override-<uuid>.*` before writing, so fetching first
/// keeps the disk-write/cleanup decision race-free. If the upsert fails,
/// the just-written file is cleaned up ONLY when no prior override cover existed
/// — otherwise the write step already replaced the user's previous valid cover
/// and cleanup would compound the loss.
async fn persist_cover(
    state: &AppState,
    uuid: &str,
    user_id: i64,
    mime: String,
    bytes: Bytes,
) -> Result<(), Response> {
    let (existing_overrides, had_prior_cover_override) =
        match db::get_metadata_overrides(&state.pool, uuid).await {
            Ok(Some((ov, has_cover))) => (ov, has_cover),
            Ok(None) => (MetadataOverrides::default(), false),
            Err(e) => return Err(internal("get_metadata_overrides", e)),
        };

    // `write_override_cover` is a sync `std::fs` call — run it on the blocking
    // pool so the axum runtime stays responsive while we hit disk (#106).
    let uuid_for_write = uuid.to_string();
    let write_result = tokio::task::spawn_blocking(move || {
        db::write_override_cover(&uuid_for_write, &mime, &bytes)
    })
    .await;
    match write_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(internal("write_override_cover", e)),
        Err(e) => return Err(internal("spawn_blocking(write_override_cover)", e)),
    }

    if let Err(e) =
        db::upsert_metadata_overrides(&state.pool, uuid, &existing_overrides, true, user_id).await
    {
        if !had_prior_cover_override {
            cleanup_orphan_cover(uuid).await;
        }
        return Err(internal("upsert_metadata_overrides", e));
    }
    Ok(())
}

/// Admin-only: bake every book's active metadata/cover overrides into its
/// EPUB container in one pass (#959) — the fleet-wide sibling of the
/// per-book export bake `get_ebook_download` already performs on demand.
/// Skips books without an active override or without an EPUB; a per-book
/// failure is collected in the returned summary rather than aborting the
/// run. Mobile-facing REST; the web counterpart is `rpc_rewrite_all_epubs`.
pub(super) async fn post_rewrite_all_epubs(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Response {
    match db::rewrite_all_epubs_with_overrides(&state.pool).await {
        Ok(summary) => Json(summary).into_response(),
        Err(e) => internal("rewrite_all_epubs", e),
    }
}

/// Best-effort delete of the on-disk override cover after a DB failure in
/// `post_ebook_cover`. `delete_override_cover` is a synchronous `std::fs`
/// call (matches the neighbouring delete-handler pattern) so it runs on
/// the blocking pool to keep the axum runtime responsive. The inner
/// call swallows its own filesystem errors; a `JoinError` here is logged
/// but ignored — the caller is already returning the original DB error,
/// and a missing cover file is preferred over a dangling file with no row
/// pointing at it.
async fn cleanup_orphan_cover(uuid: &str) {
    tracing::warn!(uuid, "cover upload DB step failed — removing orphan file");
    let uuid = uuid.to_string();
    if let Err(e) = tokio::task::spawn_blocking(move || db::delete_override_cover(&uuid)).await {
        tracing::warn!(error = %e, "spawn_blocking(delete_override_cover) join failed");
    }
}

#[cfg(test)]
mod tests;
