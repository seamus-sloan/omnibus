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
use omnibus_db::{self as db, worker::Task};
use omnibus_shared::{detect_image_format, ExternalBookMeta, MetadataOverrides};
use serde::Deserialize;

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

/// JSON body for [`post_ebook_cover_from_url`]. Inline for the same reason
/// [`super::author_photos::AuthorPhotoUrlBody`] is: one field, one call site.
#[derive(Debug, Deserialize)]
pub(super) struct CoverFromUrlBody {
    url: String,
}

/// Apply a provider's cover by URL — the metadata editor's compare view
/// pressing the arrow on the cover row.
///
/// The browser can't fetch a provider's image cross-origin to hand us bytes,
/// so the server fetches it, which is the one place in this feature with a
/// real security surface. Every gate lives in
/// [`db::provider_cover_image_config`] and the fetch it configures: HTTPS
/// only, hosts limited to the provider catalog's own (the same list the
/// `img-src` CSP is built from), each redirect hop re-checked against both
/// rather than trusted because the first hop was allowed, the IP-range guard
/// before any connect, and a size cap that bounds memory whatever the origin
/// advertises. The bytes are then sniffed here — an HTML error page served
/// with an `image/*` content-type must not land on disk as a cover.
///
/// **Rate-limited** (mounted in `upload_router`): it carries no upload body,
/// which is what keeps the other cover routes outside that limiter, but it is
/// the only override route that spends an *outbound* fetch, and that is the
/// cost worth bounding per IP.
///
/// After the fetch it is the multipart upload's tail unchanged —
/// `persist_cover`, `has_cover_override`, thumbnail invalidation — so
/// "revert to scanned cover" works on a provider cover exactly as it does on
/// an uploaded one.
pub(super) async fn post_ebook_cover_from_url(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(body): Json<CoverFromUrlBody>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let url = body.url.trim();
    if url.is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, "url is required").into_response();
    }
    // Capped before the string reaches the fetch pipeline, sharing the cap
    // `ExternalBookMeta` already applies to a provider `cover_url` — which is
    // where every URL this route legitimately receives comes from.
    if url.len() > ExternalBookMeta::COVER_URL_MAX_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            format!(
                "url must be {} bytes or fewer",
                ExternalBookMeta::COVER_URL_MAX_LEN
            ),
        )
            .into_response();
    }

    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    let config = cover_fetch_config(&state);
    let (advertised_mime, bytes) =
        match db::author_photos::fetch_remote_image_with(url, &config).await {
            Ok(pair) => pair,
            Err(db::author_photos::FetchRemoteImageError::Http(e)) => {
                // A transport failure against the *provider* is not this server
                // erroring; 502 says whose fault it was.
                tracing::warn!(error = ?e, "provider cover fetch failed");
                return (
                    axum::http::StatusCode::BAD_GATEWAY,
                    "could not fetch the cover from that source",
                )
                    .into_response();
            }
            // Everything else is a refusal we made: bad scheme, host off the
            // allowlist, blocked address, non-image content-type, too large.
            Err(e) => return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response(),
        };

    // Magic bytes, not the remote Content-Type: a provider serving an HTML
    // error page under `image/jpeg` must not be written as `override-<uuid>.jpg`
    // and served back as a cover.
    let mime = match detect_image_format(&bytes) {
        Some(m) => m,
        None => {
            tracing::warn!(
                advertised_mime,
                "provider cover URL returned image content-type but bytes are not an image"
            );
            return (
                axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                "file at URL does not appear to be a valid image",
            )
                .into_response();
        }
    };

    if let Err(response) = persist_cover(&state, &uuid, user.id, mime, Bytes::from(bytes)).await {
        return response;
    }
    if let Err(e) = tokio::task::spawn_blocking(move || db::thumbs::invalidate_thumbs(id)).await {
        return internal("spawn_blocking(invalidate_thumbs)", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// The terms [`post_ebook_cover_from_url`]'s fetch runs under.
///
/// Production is [`db::provider_cover_image_config`] unchanged: HTTPS only,
/// hosts limited to the provider catalog's, private address ranges blocked.
///
/// **One flag relaxes it, and only for tests.** `AppState`'s injectable
/// `RemoteImageConfig` (see `AppState::new_with_remote_image_config`) exists
/// so an integration test can drive a `wiremock` origin bound to
/// `127.0.0.1`, which is plaintext and private by nature — so the same flag
/// that permits the address also permits the scheme and adds the loopback
/// hosts, rather than each being a separate knob a production path could
/// trip independently. A production `AppState` is built with `Default`, whose
/// flag is `false`, so none of it applies; `cover_fetch_config_is_strict_by_default`
/// is what holds that true.
fn cover_fetch_config(state: &AppState) -> db::author_photos::RemoteImageConfig {
    let loopback_testing = state.remote_image_config().allow_private_addresses;
    let mut config = db::provider_cover_image_config(loopback_testing);
    if loopback_testing {
        config.require_https = false;
        config.host_allowlist.push("127.0.0.1".to_string());
        config.host_allowlist.push("localhost".to_string());
    }
    config
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

/// Admin-only: queue a fleet-wide bake of every book's active metadata/cover
/// overrides into its EPUB container (#959, #1718) — the bulk sibling of the
/// per-book export bake `get_ebook_download` already performs on demand.
/// Dispatches `Task::RewriteAllEpubs` to the shared worker and returns as
/// soon as it's queued, mirroring `post_scan_library`, instead of awaiting
/// the whole pass inline on the request. 200 once queued. Mobile-facing
/// REST; the web counterpart is `rpc_rewrite_all_epubs`.
pub(super) async fn post_rewrite_all_epubs(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Response {
    state.worker().post(Task::RewriteAllEpubs);
    axum::http::StatusCode::OK.into_response()
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
