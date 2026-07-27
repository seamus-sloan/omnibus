//! Kobo Reading Services — the wireless annotation channel (#1278), served at
//! the bare origin (`/api/v3/content/*`) because `reading_services_host` in
//! the initialization map is a host, not a path. No session and no path
//! token: every route authenticates via the `x-kobo-deviceid` header
//! ([`KoboHardwareUser`]). Exempted from `require_auth` in `auth::gate`.
//!
//! Protocol quirks this module honors: the device never echoes a real ETag
//! (always `If-None-Match: W/"0"`) and drives change detection through
//! `checkforchanges`; deletions propagate **by omission** (anything absent
//! from a GET body is removed on-device — tombstone flags are ignored); and
//! a (device, book) pair is answered `304` until its first clean PATCH, so a
//! first sync can never wipe highlights made before wireless sync existed.

use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Extension, Json, Router,
};
use futures_util::stream;
use omnibus_db as db;
use serde_json::Value;

use super::extractor::KoboHardwareUser;
use super::{internal, reject_oversized_uuid, AppState};

mod dto;
#[cfg(test)]
mod tests;

/// Body-size ceiling for a device PATCH. The first PATCH after adoption can
/// carry a whole pre-wireless backlog, which the global limit would reject.
const PATCH_BODY_LIMIT: usize = 8 * 1024 * 1024;

/// Build the Reading Services router. Self-contained for `oneshot` tests,
/// mirroring `kobo_router`.
pub fn reading_services_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/api/v3/content/checkforchanges", post(check_for_changes))
        .route(
            "/api/v3/content/{content_id}/annotations",
            get(get_annotations).patch(patch_annotations),
        )
        .route("/api/UserStorage/Metadata", get(user_storage_metadata))
        .layer(DefaultBodyLimit::max(PATCH_BODY_LIMIT))
        .with_state(state)
        .layer(Extension(pool))
}

/// `GET .../content/{contentId}/annotations` — serve the book's Kobo-placeable
/// annotation set with a weak content-hash ETag.
///
/// The per-(device, book) ack advances in the stream's final poll — the same
/// pattern as `library_sync`'s snapshot commit: a connection dropped mid-body
/// never acks, so `checkforchanges` re-reports the book, which is the
/// protocol's only redelivery mechanism.
async fn get_annotations(
    auth: KoboHardwareUser,
    State(state): State<AppState>,
    Path(content_id): Path<String>,
    headers: HeaderMap,
) -> Response {
    let uuid = dto::parse_content_id(&content_id);
    if let Some(reject) = reject_oversized_uuid(uuid) {
        return reject;
    }
    let canonical = match db::resolve_canonical_book_uuid(state.pool(), uuid).await {
        Ok(Some(c)) => c,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("reading-services resolve uuid", e),
    };

    let served = match db::annotations::served_kobo_annotations(
        state.pool(),
        auth.user_id,
        &canonical,
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return internal("reading-services served set", e),
    };

    // A pair no device has uploaded for — and with nothing to serve from
    // anywhere else — must not receive an empty-but-valid set: the device
    // reconciles by omission, so that answer would erase its pre-wireless
    // backlog (AC5). A non-empty set is safe for any device, which is what
    // lets a second or factory-reset device receive existing annotations.
    let adopted =
        match db::kobo::annotations::is_adopted(state.pool(), auth.device_id, &canonical).await {
            Ok(a) => a || !served.is_empty(),
            Err(e) => return internal("reading-services adoption check", e),
        };
    if !adopted {
        return (
            StatusCode::NOT_MODIFIED,
            [(header::ETAG, "W/\"0\"".to_string())],
        )
            .into_response();
    }

    let fingerprint = db::kobo::annotations::fingerprint(&served);
    let etag = format!("W/\"{fingerprint}\"");

    // Firmware always sends `If-None-Match: W/"0"`, so this arm is
    // theoretical — but a correct 304 is cheap, and it still acks: a matched
    // ETag means the device already holds exactly this set.
    if if_none_match_matches(&headers, &etag) {
        if let Err(e) = db::kobo::annotations::ack_served(
            state.pool(),
            auth.device_id,
            &canonical,
            &fingerprint,
        )
        .await
        {
            tracing::warn!(error = %e, "reading-services 304 ack failed");
        }
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }

    let response = dto::AnnotationsResponse {
        annotations: served.iter().map(dto::AnnotationOut::from_served).collect(),
        next_page_offset_token: None,
    };
    let body = match serde_json::to_vec(&response) {
        Ok(b) => Bytes::from(b),
        Err(e) => return internal("reading-services serialize", e),
    };

    let pool = state.pool().clone();
    let device_id = auth.device_id;
    let stream = stream::unfold(
        (Some(body), canonical, fingerprint, false),
        move |(body, canonical, fingerprint, acked)| {
            let pool = pool.clone();
            async move {
                match body {
                    // First poll: the whole body in one chunk.
                    Some(bytes) => Some((
                        Ok::<Bytes, std::io::Error>(bytes),
                        (None, canonical, fingerprint, acked),
                    )),
                    // Second poll — only reached once the chunk has drained:
                    // record what this device now holds. Non-fatal on error;
                    // the book just stays in checkforchanges.
                    None if !acked => {
                        if let Err(e) = db::kobo::annotations::ack_served(
                            &pool,
                            device_id,
                            &canonical,
                            &fingerprint,
                        )
                        .await
                        {
                            tracing::warn!(device_id, error = %e, "reading-services ack failed");
                        }
                        Some((Ok(Bytes::new()), (None, canonical, fingerprint, true)))
                    }
                    None => None,
                }
            }
        },
    );

    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json".to_string()),
            (header::ETAG, etag),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

/// `PATCH .../content/{contentId}/annotations` — ingest device-created
/// highlights, notes, and deletes. Always `204` for well-addressed requests;
/// device input is untrusted, so unparseable entries are skipped (and block
/// adoption) rather than erroring the sync loop.
async fn patch_annotations(
    auth: KoboHardwareUser,
    State(state): State<AppState>,
    Path(content_id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let uuid = dto::parse_content_id(&content_id);
    if let Some(reject) = reject_oversized_uuid(uuid) {
        return reject;
    }
    let parsed = dto::parse_patch(&body);
    if parsed.skipped > 0 {
        tracing::warn!(
            skipped = parsed.skipped,
            "reading-services PATCH skipped unparseable entries"
        );
    }

    match db::annotations::ingest_kobo_annotations(
        state.pool(),
        auth.user_id,
        uuid,
        &parsed.updates,
        &parsed.deleted_ids,
    )
    .await
    {
        Ok(()) => {}
        // An id this library can't resolve is device noise (a sideloaded or
        // since-purged book) — erroring would just make the device retry it
        // forever.
        Err(db::annotations::HighlightError::BookNotFound) => {
            tracing::warn!(content_id = %content_id, "reading-services PATCH for unknown book");
            return StatusCode::NO_CONTENT.into_response();
        }
        Err(e) => return internal("reading-services ingest", e),
    }

    // Adopt only after a fully-clean, non-empty upload: adopting a PATCH we
    // partially dropped would let the next GET's omission wipe what we
    // skipped, and an empty PATCH proves nothing about the device's backlog.
    if parsed.skipped == 0 && (!parsed.updates.is_empty() || !parsed.deleted_ids.is_empty()) {
        let canonical = match db::resolve_canonical_book_uuid(state.pool(), uuid).await {
            Ok(Some(c)) => c,
            Ok(None) => return StatusCode::NO_CONTENT.into_response(),
            Err(e) => return internal("reading-services resolve for adoption", e),
        };
        if let Err(e) =
            db::kobo::annotations::mark_adopted(state.pool(), auth.device_id, &canonical).await
        {
            return internal("reading-services adopt", e);
        }
    }
    StatusCode::NO_CONTENT.into_response()
}

/// `POST .../content/checkforchanges` — the entitlement ids whose annotation
/// sets moved past what this device last acked. The request body (the
/// device's own id/etag list) is deliberately never parsed: the server-side
/// watermark is authoritative, and shape drift must not 400 the sync loop.
async fn check_for_changes(
    auth: KoboHardwareUser,
    State(state): State<AppState>,
    _body: Bytes,
) -> Response {
    match db::kobo::annotations::changed_book_uuids(state.pool(), auth.user_id, auth.device_id)
        .await
    {
        Ok(uuids) => Json(uuids).into_response(),
        Err(e) => internal("reading-services checkforchanges", e),
    }
}

/// `GET /api/UserStorage/Metadata` — firmware polls this on the reading
/// services host; an empty inventory keeps it quiet. Ten lines beats an
/// error-retry loop in the device log.
async fn user_storage_metadata(_auth: KoboHardwareUser) -> Response {
    Json(serde_json::json!({ "continuationToken": null, "metadata": [] })).into_response()
}

/// Does the request's `If-None-Match` name exactly this ETag?
fn if_none_match_matches(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|raw| raw.split(',').any(|candidate| candidate.trim() == etag))
}
