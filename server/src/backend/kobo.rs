//! Native Kobo wireless sync (`/kobo/<TOKEN>/v1/*`).
//!
//! Mounted outside the `/api/*` auth gate; each route authenticates via its
//! path token ([`KoboAuthUser`]). Serves the initialization handshake, the
//! library enumeration, per-book metadata, KEPUB download, the read-state PUT,
//! tags, and cover images.

use axum::{
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use futures_util::stream;
use omnibus_db::{
    self as db,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::{ProgressFormat, ProgressUpdate, ReadStatus, SetReadStatus};

use super::{internal, serve_download, AppState};

mod analytics;
mod dto;
mod extractor;
mod reading_services;
mod store_resources;
#[cfg(test)]
mod tests;

use extractor::KoboAuthUser;
pub use reading_services::reading_services_router;

/// Wall-clock budget for an inline KEPUB conversion on a Kobo download before
/// falling back to plain EPUB. Mirrors the USB sideload path's budget.
const KEPUB_CONVERT_BUDGET: std::time::Duration = std::time::Duration::from_secs(25);

/// Required on the `v1/initialization` response — base64 `{}`. Without it the
/// device treats the payload as malformed and never adopts the resources map.
const KOBO_API_TOKEN: &str = "e30=";

/// Changes per `library/sync` response; more remain → `x-kobo-sync: continue`, bounding the response but never the sync (unlike Calibre-Web's `SYNC_ITEM_LIMIT`).
const SYNC_PAGE_SIZE: usize = 100;

/// Build the wireless Kobo router. `Extension(pool)` is layered here so the
/// router is self-contained for integration tests; the live server adds the
/// same one at the top (harmless overlap, mirroring `rest_router`).
pub fn kobo_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/kobo/{token}/v1/initialization", get(initialization))
        .route("/kobo/{token}/v1/auth/device", post(auth_device))
        .route("/kobo/{token}/v1/auth/refresh", post(auth_refresh))
        .route(
            "/kobo/{token}/v1/analytics/event",
            post(analytics::analytics_event),
        )
        .route(
            "/kobo/{token}/v1/analytics/gettests",
            get(analytics::analytics_gettests),
        )
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

/// `GET v1/initialization` — the handshake that redirects a device at this
/// server. Returns Kobo's own resources map with only the sync/download/cover/
/// annotation entries repointed here, so store browse and search keep working
/// against Kobo directly and this server never proxies that traffic.
///
/// `reading_services_host` points the device's annotation channel here too —
/// answered by [`reading_services::reading_services_router`] at the bare
/// origin (#1278), since the device calls it without the path token.
async fn initialization(auth: KoboAuthUser, headers: HeaderMap) -> Response {
    let base = origin_from_headers(&headers);
    let resources = store_resources::resources_for(&base, &auth.token);
    (
        StatusCode::OK,
        [(
            header::HeaderName::from_static("x-kobo-apitoken"),
            KOBO_API_TOKEN,
        )],
        Json(serde_json::json!({ "Resources": resources })),
    )
        .into_response()
}

/// `POST v1/auth/device` — the device's initial token exchange. The values are
/// generated locally and never validated afterwards: the `/kobo/<TOKEN>/` path
/// token is the real credential, so this is a well-formed envelope by design,
/// not a stub standing in for verification.
async fn auth_device(auth: KoboAuthUser) -> Response {
    Json(dto::auth_envelope(&auth.token)).into_response()
}

/// `POST v1/auth/refresh` — same envelope as [`auth_device`]; the device
/// refreshes on a schedule and expects the same shape back.
async fn auth_refresh(auth: KoboAuthUser) -> Response {
    Json(dto::auth_envelope(&auth.token)).into_response()
}

/// `GET library/sync` — the per-device delta, streamed as a JSON array via
/// [`Body::from_stream`] with no item cap (unlike Calibre-Web's
/// `SYNC_ITEM_LIMIT=100`). First sync emits the whole opted-in set as
/// `NewEntitlement`s; later syncs emit only `ChangedProductMetadata` +
/// `ChangedReadingState` for modified books and `ChangedEntitlement
/// {IsRemoved:true}` for books that left the opted-in set (#922).
///
/// The device's snapshot advances in the stream's final poll — the same one
/// that emits the closing `]`, so commit and completion are a single step the
/// transport can't split. A connection dropped mid-body never reaches that
/// poll, and the device replays the same delta on the next sync instead of
/// silently losing books.
///
/// Responses are paged at [`SYNC_PAGE_SIZE`] changes: only the first page is
/// emitted (and committed), and `x-kobo-sync: continue` tells the device to
/// re-hit the route for the rest. Pagination needs no extra cursor — a
/// committed page is in the snapshot, so the next request's delta *is* the
/// remainder.
async fn library_sync(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    let delta = match db::kobo::sync_delta(state.pool(), auth.user_id, auth.device_id).await {
        Ok(d) => d,
        Err(e) => return internal("kobo sync_delta", e),
    };
    let base = origin_from_headers(&headers);
    let pool = state.pool().clone();
    let device_id = auth.device_id;

    let mut changes = delta.changes;
    let has_more = changes.len() > SYNC_PAGE_SIZE;
    changes.truncate(SYNC_PAGE_SIZE);

    // Load the per-user reading state for this page only — the emit needs real
    // status/position per book, and a removal has no state to report.
    let page_uuids: Vec<String> = changes
        .iter()
        .filter_map(|c| match c {
            db::kobo::SyncChange::New(b)
            | db::kobo::SyncChange::Changed(b)
            | db::kobo::SyncChange::StateChanged(b) => Some(b.uuid.clone()),
            db::kobo::SyncChange::Removed { .. } => None,
        })
        .collect();
    let states = match db::kobo::reading_state_for(state.pool(), auth.user_id, &page_uuids).await {
        Ok(s) => s,
        Err(e) => return internal("kobo reading_state_for", e),
    };
    let delta = db::kobo::SyncDelta { changes };

    // Serialize each item lazily as the device drains the body — no second
    // buffer of the whole payload.
    let stream = stream::unfold(
        (delta.changes, base, auth.token, states, SyncPhase::Open),
        move |(changes, base, token, states, phase)| {
            let pool = pool.clone();
            async move {
                let chunk: Bytes = match phase {
                    SyncPhase::Open => {
                        let next = if changes.is_empty() {
                            SyncPhase::Close
                        } else {
                            SyncPhase::Item {
                                change: 0,
                                emitted: 0,
                            }
                        };
                        return Some((
                            ok(Bytes::from_static(b"[")),
                            (changes, base, token, states, next),
                        ));
                    }
                    SyncPhase::Item { change, emitted } => {
                        let uuid = match &changes[change] {
                            db::kobo::SyncChange::New(b)
                            | db::kobo::SyncChange::Changed(b)
                            | db::kobo::SyncChange::StateChanged(b) => Some(b.uuid.as_str()),
                            db::kobo::SyncChange::Removed { .. } => None,
                        };
                        let book_state = uuid.and_then(|u| states.get(u));
                        let items = dto::sync_items(&base, &token, &changes[change], book_state);
                        let mut piece = Vec::new();
                        for item in &items {
                            // These DTOs are owned Strings/primitives, so this
                            // never fails in practice — but if it ever does,
                            // abort the body rather than emit malformed JSON:
                            // the stream jumps to End, `record_synced` never
                            // runs, and the device retries the same delta.
                            let json = match serde_json::to_vec(item) {
                                Ok(j) => j,
                                Err(e) => {
                                    return Some((
                                        Err(std::io::Error::other(e)),
                                        (changes, base, token, states, SyncPhase::End),
                                    ));
                                }
                            };
                            if emitted > 0 || !piece.is_empty() {
                                piece.push(b',');
                            }
                            piece.extend_from_slice(&json);
                        }
                        let next = if change + 1 < changes.len() {
                            SyncPhase::Item {
                                change: change + 1,
                                emitted: emitted + items.len(),
                            }
                        } else {
                            SyncPhase::Close
                        };
                        return Some((
                            ok(Bytes::from(piece)),
                            (changes, base, token, states, next),
                        ));
                    }
                    SyncPhase::Close => {
                        // Body is complete: commit the snapshot. A failure here
                        // is deliberately non-fatal to the response (the bytes
                        // are already on the wire) — the device just replays
                        // the same delta next sync, which is safe.
                        if let Err(e) = db::kobo::record_synced(&pool, device_id, &changes).await {
                            tracing::warn!(device_id, error = %e, "kobo snapshot advance failed");
                        }
                        Bytes::from_static(b"]")
                    }
                    SyncPhase::End => return None,
                };
                Some((ok(chunk), (changes, base, token, states, SyncPhase::End)))
            }
        },
    );

    let mut res = (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "application/json"),
            // Opaque; the device echoes it back but the real cursor is the
            // per-device snapshot (`kobo_books_sync`), never this value.
            (
                header::HeaderName::from_static("x-kobo-synctoken"),
                "omnibus",
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response();
    if has_more {
        res.headers_mut().insert(
            header::HeaderName::from_static("x-kobo-sync"),
            header::HeaderValue::from_static("continue"),
        );
    }
    res
}

/// Streaming position for [`library_sync`]'s JSON-array body. `emitted`
/// counts items already written, since one change can fan out into two items.
enum SyncPhase {
    Open,
    Item { change: usize, emitted: usize },
    Close,
    End,
}

/// Wrap a chunk as the infallible `Result` item `Body::from_stream` expects.
fn ok(bytes: Bytes) -> Result<Bytes, std::io::Error> {
    Ok(bytes)
}

/// Cap a path `{uuid}` at [`omnibus_shared::BOOK_UUID_MAX_LEN`] before any DB
/// round trip, mirroring the request-input sweep the JSON-body routes already
/// follow (`kindle::SendBody::validate`). `Some(response)` is the rejection.
fn reject_oversized_uuid(uuid: &str) -> Option<Response> {
    (uuid.len() > omnibus_shared::BOOK_UUID_MAX_LEN)
        .then(|| StatusCode::BAD_REQUEST.into_response())
}

/// `GET library/<uuid>/metadata` — the single-book metadata array Kobo fetches
/// before downloading.
async fn library_metadata(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
    match db::kobo::book_for_sync(state.pool(), &uuid).await {
        Ok(Some(book)) => {
            let base = origin_from_headers(&headers);
            Json(vec![dto::book_metadata(&base, &auth.token, &book)]).into_response()
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
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
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
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
    for entry in &body.reading_states {
        if let Some(info) = &entry.status_info {
            let update = SetReadStatus {
                book_uuid: uuid.clone(),
                status: map_status(&info.status),
            };
            match db::read_status::set_read_status(state.pool(), auth.user_id, &update).await {
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
        if let Some(bookmark) = &entry.current_bookmark {
            match persist_bookmark(&state, auth.user_id, &uuid, bookmark).await {
                Ok(()) => {}
                Err(db::progress::ProgressError::BookNotFound) => {
                    tracing::warn!(%uuid, "kobo position PUT for unknown book");
                }
                Err(db::progress::ProgressError::Sqlx(e)) => {
                    return internal("kobo upsert_progress", e);
                }
            }
        }
        // Acknowledged, deliberately unwritten — cumulative totals would
        // double-count against the LeaveContent sessions (see `dto::Statistics`).
        if entry.statistics.is_some() {
            tracing::debug!(%uuid, "kobo statistics received");
        }
    }
    Json(dto::StateResponse::success(uuid)).into_response()
}

/// Persist a device's `CurrentBookmark` as an epub position (#925).
///
/// The Kobo location is a `KoboSpan`, not a CFI, so it rides in its own opaque
/// column and never touches `epub_cfi`; the percent is the half that means the
/// same thing on every surface. A bookmark carrying neither is a no-op rather
/// than a validation error — the device sends the field either way.
async fn persist_bookmark(
    state: &AppState,
    user_id: i64,
    uuid: &str,
    bookmark: &dto::CurrentBookmark,
) -> Result<(), db::progress::ProgressError> {
    // The device reports whole-book percent in `ProgressPercent`; the
    // content-source variant is per-chapter and not comparable across surfaces.
    //
    // An out-of-range value is *dropped*, not clamped: clamping would turn
    // data we plainly don't understand into a confident-looking resume point
    // (101 → "finished"), and a silently wrong position is worse than none.
    let percent = bookmark.progress_percent.filter(|p| (0..=100).contains(p));
    let location = bookmark
        .location
        .as_ref()
        .map(|l| serde_json::to_string(l).unwrap_or_default())
        .filter(|s| !s.is_empty());
    if percent.is_none() && location.is_none() {
        return Ok(());
    }
    let update = ProgressUpdate {
        book_uuid: uuid.to_owned(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: percent,
        kobo_location: location,
        // Epub-format position; `book_file_id` selects among multiple audio
        // files and has no meaning here.
        book_file_id: None,
        client_updated_at: Some(time::OffsetDateTime::now_utc().unix_timestamp()),
    };
    // A location with no percent can't satisfy the epub position rule, and
    // there is nothing to store for the web reader in that case either.
    if update.validate().is_err() {
        tracing::debug!(%uuid, "kobo bookmark carried no usable position");
        return Ok(());
    }
    db::progress::upsert_progress(state.pool(), user_id, &update).await?;
    Ok(())
}

/// `GET library/tags` — device-side collections. Slice A returns an empty set;
/// shelves-as-collections is #924.
async fn library_tags(_auth: KoboAuthUser) -> Response {
    Json(serde_json::json!([])).into_response()
}

/// `GET books/<uuid>/thumbnail/.../image.jpg` — serve the book cover. The
/// requested dimensions are advisory; the stored cover is served as-is.
///
/// Carries a weak `ETag` derived from `(book id, last_modified)` and honors
/// `If-None-Match` with a bodyless 304 — the device re-validates covers on
/// every sync, so without this each sync re-downloads every cover it already
/// holds.
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
    headers: HeaderMap,
) -> Response {
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
    let id = match db::resolve_book_id_by_uuid(state.pool(), &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("kobo image resolve", e),
    };
    // Cheap freshness probe before the cover bytes are ever loaded: the cover
    // changes only through paths that bump `books.last_modified` (override
    // save, reindex), so the pair is an honest validator.
    let last_modified: i64 = match sqlx::query_scalar(
        "SELECT CAST(COALESCE(last_modified, 0) AS INTEGER) FROM books WHERE id = ?",
    )
    .bind(id)
    .fetch_one(state.pool())
    .await
    {
        Ok(lm) => lm,
        Err(e) => return internal("kobo image last_modified", e),
    };
    let etag = format!("W/\"{id}-{last_modified}\"");
    if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.split(',').any(|c| c.trim() == etag))
    {
        return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
    }
    match db::get_cover(state.pool(), id).await {
        Ok(Some((mime, bytes))) => {
            ([(header::CONTENT_TYPE, mime), (header::ETAG, etag)], bytes).into_response()
        }
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

/// Reconstruct the request origin (`scheme://host`) so download/image URLs are
/// absolute. Prefers `X-Forwarded-Host` (reverse proxy) over `Host`, and
/// `X-Forwarded-Proto` over a `http` default. When no host is resolvable,
/// returns an empty string so callers emit host-relative URLs rather than an
/// invalid `http:///…`.
fn origin_from_headers(headers: &HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty());
    let Some(host) = host else {
        return String::new();
    };
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}
