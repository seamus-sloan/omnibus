//! Native Kobo wireless sync (`/kobo/<TOKEN>/v1/*`).
//!
//! Mounted outside the `/api/*` auth gate; each route authenticates via its
//! path token ([`KoboAuthUser`]). Serves the initialization handshake, the
//! library enumeration, per-book metadata, KEPUB download, the read-state
//! GET/PUT, tags, and cover images.

use axum::{
    body::{Body, Bytes},
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Extension, Json, Router,
};
use futures_util::stream;
use omnibus_db::{
    self as db,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::{ProgressFormat, ProgressUpdate, ReadStatus, SetReadStatus};
use sqlx::{Sqlite, Transaction};

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
        // Real firmware POSTs gettests despite it being a fetch; serve both.
        .route(
            "/kobo/{token}/v1/analytics/gettests",
            get(analytics::analytics_gettests).post(analytics::analytics_gettests),
        )
        .route("/kobo/{token}/v1/library/sync", get(library_sync))
        .route(
            "/kobo/{token}/v1/library/{uuid}/metadata",
            get(library_metadata),
        )
        .route(
            "/kobo/{token}/v1/library/{uuid}/state",
            get(get_state).put(put_state),
        )
        .route("/kobo/{token}/v1/library/tags", get(library_tags))
        .route("/kobo/{token}/v1/download/{uuid}", get(download))
        .route(
            "/kobo/{token}/v1/books/{uuid}/thumbnail/{w}/{h}/{quality}/{greyscale}/image.jpg",
            get(image),
        )
        // Registered routes win over the wildcard; only unhandled paths land here.
        .route("/kobo/{token}/{*rest}", any(store_stub))
        .with_state(state)
        .layer(Extension(pool))
}

/// Benign `200 {}` for store paths the firmware derives from `api_endpoint`
/// itself (`v1/user/profile`, `v1/deals`, …), bypassing the initialization
/// resources map that points them at Kobo. A 404 on any of them makes the
/// device abort the whole sync before `library/sync`; Calibre-Web answers the
/// same paths with an empty object. The log line doubles as capture data for
/// the #928 golden fixture.
async fn store_stub(auth: KoboAuthUser, Path((_token, rest)): Path<(String, String)>) -> Response {
    // `?rest` (Debug) escapes control chars the router percent-decodes into
    // the path; device_id makes multi-device captures attributable.
    tracing::info!(
        device_id = auth.device_id,
        path = ?rest,
        "kobo store path answered with empty stub"
    );
    Json(serde_json::json!({})).into_response()
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
    let mut states =
        match db::kobo::reading_state_for(state.pool(), auth.user_id, &page_uuids).await {
            Ok(s) => s,
            Err(e) => return internal("kobo reading_state_for", e),
        };
    enrich_states_with_derived_spans(&state, auth.user_id, &changes, &mut states).await;
    let states = states;
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

/// Per-sync cap on CFI→span derivations. Each one walks a whole book's
/// spine for the percent, so a page full of web-written positions must not
/// stall the sync; the clock-neutral write-back shrinks the candidate set
/// monotonically, so the remainder heals across subsequent syncs.
const SPAN_DERIVATIONS_PER_SYNC: usize = 8;

/// For page books whose freshest position is a web CFI (`kobo_location`
/// NULL, `epub_cfi` present), derive a KoboSpan + percent so the device
/// gets an exact position, and persist the derived halves clock-neutrally
/// so later syncs skip the work. Every failure leaves the state untouched —
/// the device then sees whatever percent exists, exactly as before.
async fn enrich_states_with_derived_spans(
    state: &AppState,
    user_id: i64,
    changes: &[db::kobo::SyncChange],
    states: &mut std::collections::HashMap<String, db::kobo::KoboBookState>,
) {
    let mut budget = SPAN_DERIVATIONS_PER_SYNC;
    for change in changes {
        if budget == 0 {
            break;
        }
        let book = match change {
            db::kobo::SyncChange::New(b)
            | db::kobo::SyncChange::Changed(b)
            | db::kobo::SyncChange::StateChanged(b) => b,
            db::kobo::SyncChange::Removed { .. } => continue,
        };
        let Some(s) = states.get(&book.uuid) else {
            continue;
        };
        let Some(cfi) = (s.kobo_location.is_none())
            .then(|| s.epub_cfi.clone())
            .flatten()
        else {
            continue;
        };
        budget -= 1;
        let Some(derived) = derive_span_for_cfi(state, book.id, &book.uuid, &cfi).await else {
            continue;
        };
        if let (Some(loc), Some(s)) = (&derived.location_json, states.get_mut(&book.uuid)) {
            let attach = db::progress::attach_derived_kobo_location(
                state.pool(),
                user_id,
                &book.uuid,
                loc,
                derived.percent,
                s.progress_updated_at,
            )
            .await;
            if let Err(e) = attach {
                tracing::warn!(uuid = %book.uuid, error = %e, "derived span write-back failed");
            }
            s.kobo_location = Some(loc.clone());
            s.percent = s.percent.or(derived.percent);
        } else if let Some(s) = states.get_mut(&book.uuid) {
            // Kepub half unavailable: a truthful percent still beats an
            // empty bookmark. Not persisted — the row keeps meaning "no
            // span known" so a later sync retries the full derivation.
            s.percent = s.percent.or(derived.percent);
        }
    }
}

/// Best-effort CFI→KoboSpan derivation for sync-out. The kepub cache is
/// used even when stale (content is identical across metadata rebuilds;
/// the snippet check guards real divergence); when it is absent entirely,
/// a conversion is queued fire-and-forget so a later sync can succeed, and
/// the percent half still derives from the source EPUB alone.
async fn derive_span_for_cfi(
    state: &AppState,
    book_id: i64,
    uuid: &str,
    cfi: &str,
) -> Option<db::kobo_position::DerivedSpan> {
    let source = db::book_file_path(state.pool(), book_id, "EPUB")
        .await
        .ok()??;
    let kepub = db::kepub_path(book_id);
    let kepub = if tokio::fs::try_exists(&kepub).await.unwrap_or(false) {
        Some(kepub)
    } else {
        // Idempotent and serialized per book by the worker; do NOT await —
        // the sync response must not stall on a conversion.
        state.worker().post(Task::KepubConvert { book_id });
        None
    };
    let cfi = cfi.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        db::kobo_position::cfi_to_span(kepub.as_deref(), &source, &cfi)
    })
    .await;
    match result {
        Ok(Ok(derived)) => Some(derived),
        Ok(Err(e)) => {
            tracing::warn!(%uuid, error = %e, "kobo cfi→span derivation failed");
            None
        }
        Err(e) => {
            tracing::warn!(%uuid, error = %e, "kobo cfi→span task panicked");
            None
        }
    }
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

/// Cap a `state` PUT body at [`dto::StateRequest::MAX_READING_STATES`] entries
/// before any DB work, mirroring [`reject_oversized_uuid`]. `Some(response)`
/// is the rejection.
fn reject_oversized_state_request(body: &dto::StateRequest) -> Option<Response> {
    (body.reading_states.len() > dto::StateRequest::MAX_READING_STATES)
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

/// `GET library/<uuid>/state` — the device's pull of the server-side reading
/// state for one book, the request the firmware adopts a position from: a
/// one-element array of the same `ReadingState` shape the PUT consumes and
/// `library/sync` emits (prosa-kobo contract).
///
/// Deliberately shares `library/sync`'s span enrichment, side effects
/// included: a CFI-only position goes out as an exact KoboSpan, the derived
/// halves are persisted so later syncs skip the work, and a missing kepub
/// cache queues a fire-and-forget conversion — all idempotent, mirroring
/// what the same book would get on its next `library/sync`.
async fn get_state(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
) -> Response {
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
    let book = match db::kobo::book_for_sync(state.pool(), &uuid).await {
        Ok(Some(b)) => b,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("kobo book_for_sync", e),
    };
    let mut states = match db::kobo::reading_state_for(
        state.pool(),
        auth.user_id,
        std::slice::from_ref(&book.uuid),
    )
    .await
    {
        Ok(s) => s,
        Err(e) => return internal("kobo reading_state_for", e),
    };
    let ts = dto::rfc3339(book.last_modified_epoch);
    let changes = [db::kobo::SyncChange::StateChanged(book.clone())];
    enrich_states_with_derived_spans(&state, auth.user_id, &changes, &mut states).await;
    Json(vec![dto::reading_state(
        &book.uuid,
        &ts,
        states.get(&book.uuid),
    )])
    .into_response()
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
        // Still override-baked — same fallback `get_ebook_kepub` uses — so a
        // Kobo device without kepubify support (or after a conversion
        // failure) sees the user's edits rather than the raw scanned file.
        let source = match db::book_file_path(state.pool(), id, "EPUB").await {
            Ok(Some(p)) => p,
            Ok(None) => return StatusCode::NOT_FOUND.into_response(),
            Err(e) => return internal("kobo book_file_path", e),
        };
        super::ebooks::rewritten_or_source(&state, id, source).await
    };
    serve_download(req, &path, "application/epub+zip").await
}

/// `PUT library/<uuid>/state` — persist the device's reading state.
/// `StatusInfo` routes into `book_read_status`; the `CurrentBookmark`
/// position lands in `reading_progress` as percent + verbatim `KoboSpan`
/// plus a server-derived `epub_cfi` when the cached KEPUB allows it
/// (#925), stamped with the device's own event time when it sends one.
///
/// The whole batch's read-status and bookmark writes share one transaction
/// (begun before the loop, committed after it), so a mid-batch DB failure
/// rolls back every entry rather than leaving the sync half-applied — a
/// `BookNotFound` for one entry is not such a failure and is logged and
/// skipped within the same transaction.
async fn put_state(
    auth: KoboAuthUser,
    State(state): State<AppState>,
    Path((_token, uuid)): Path<(String, String)>,
    Json(body): Json<dto::StateRequest>,
) -> Response {
    if let Some(rejected) = reject_oversized_uuid(&uuid) {
        return rejected;
    }
    if let Some(rejected) = reject_oversized_state_request(&body) {
        return rejected;
    }
    let mut tx = match state.pool().begin().await {
        Ok(tx) => tx,
        Err(e) => return internal("kobo put_state begin", e),
    };
    for entry in &body.reading_states {
        if let Some(info) = &entry.status_info {
            let update = SetReadStatus {
                book_uuid: uuid.clone(),
                status: map_status(&info.status),
            };
            match db::read_status::set_read_status_tx(&mut tx, auth.user_id, &update).await {
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
            // Capture data for the one open question about this field: whether
            // the content-source percent really is per-chapter (as
            // `persist_bookmark` assumes) or just echoes the whole-book one.
            // Nothing in this repo has ever recorded a real device's answer,
            // and sync-out can't emit the field until it is known.
            tracing::info!(
                %uuid,
                whole_book = ?bookmark.progress_percent,
                content_source = ?bookmark.content_source_progress_percent,
                "kobo bookmark percents received"
            );
            // Device event time, most-specific first: the bookmark's own
            // stamp, then the entry-level ones. Absent/unparseable → None,
            // which `persist_bookmark` treats as server-now (pre-existing
            // behaviour for firmware that sends no clock).
            let event_ts = bookmark
                .last_modified
                .as_deref()
                .or(entry.last_modified.as_deref())
                .or(entry.priority_timestamp.as_deref())
                .and_then(dto::parse_kobo_timestamp);
            match persist_bookmark(&state, &mut tx, auth.user_id, &uuid, bookmark, event_ts).await {
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
        // Logged with values because sync-out omits the block entirely today,
        // and the only safe way to start sending one is to echo what a device
        // reported rather than invent zeroes; that needs a real payload first.
        if let Some(stats) = &entry.statistics {
            tracing::info!(
                %uuid,
                spent_reading_minutes = ?stats.spent_reading_minutes,
                remaining_time_minutes = ?stats.remaining_time_minutes,
                "kobo statistics received"
            );
        }
    }
    if let Err(e) = tx.commit().await {
        return internal("kobo put_state commit", e);
    }
    Json(dto::StateResponse::success(uuid)).into_response()
}

/// Persist a device's `CurrentBookmark` as an epub position (#925).
///
/// The `KoboSpan` location rides verbatim in its own column, and — when the
/// cached KEPUB allows — a CFI derived from it lands in `epub_cfi`, so the
/// web/iOS readers resume at the same sentence with no client changes. The
/// percent is the device's own number and is never overwritten by a derived
/// one. `event_ts` is the device's clock (`None` → server-now); the upsert's
/// forward clamp bounds a fast device clock. A bookmark carrying neither
/// percent nor location is a no-op rather than a validation error — the
/// device sends the field either way.
///
/// The final write goes through `tx` so it shares [`put_state`]'s batch
/// transaction; the CFI derivation above it only reads (via `state.pool()`),
/// so it stays outside the transaction rather than blocking it.
async fn persist_bookmark(
    state: &AppState,
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    uuid: &str,
    bookmark: &dto::CurrentBookmark,
    event_ts: Option<i64>,
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
    let epub_cfi = match &location {
        Some(loc) => derive_cfi_for_location(state, uuid, loc).await,
        None => None,
    };
    let update = ProgressUpdate {
        book_uuid: uuid.to_owned(),
        format: ProgressFormat::Epub,
        epub_cfi,
        audio_position_seconds: None,
        progress_percent: percent,
        kobo_location: location,
        // Epub-format position; `book_file_id` selects among multiple audio
        // files and has no meaning here.
        book_file_id: None,
        client_updated_at: Some(
            event_ts.unwrap_or_else(|| time::OffsetDateTime::now_utc().unix_timestamp()),
        ),
    };
    // A location with no percent (and no derived CFI) can't satisfy the epub
    // position rule, and there is nothing to store for any reader either.
    if update.validate().is_err() {
        tracing::debug!(%uuid, "kobo bookmark carried no usable position");
        return Ok(());
    }
    db::progress::upsert_progress_tx(tx, user_id, &update).await?;
    Ok(())
}

/// Best-effort KoboSpan→CFI derivation for a state PUT. Uses the cached
/// KEPUB **even when stale** — metadata edits rebuild the cache without
/// touching content, and the snippet check inside `span_to_cfi` guards the
/// case where the source file really was replaced. No staleness check, no
/// worker enqueue: the state PUT stays fast, and a `None` simply means the
/// row stores percent + span exactly as before.
async fn derive_cfi_for_location(
    state: &AppState,
    uuid: &str,
    location_json: &str,
) -> Option<String> {
    let loc = db::kobo_position::parse_location(location_json)?;
    let book_id = db::resolve_book_id_by_uuid(state.pool(), uuid)
        .await
        .ok()??;
    let kepub = db::kepub_path(book_id);
    if !tokio::fs::try_exists(&kepub).await.unwrap_or(false) {
        return None;
    }
    let source = db::book_file_path(state.pool(), book_id, "EPUB")
        .await
        .ok()??;
    let result =
        tokio::task::spawn_blocking(move || db::kobo_position::span_to_cfi(&kepub, &source, &loc))
            .await;
    match result {
        Ok(Ok(cfi)) => cfi,
        Ok(Err(e)) => {
            tracing::warn!(%uuid, error = %e, "kobo span→cfi derivation failed");
            None
        }
        Err(e) => {
            tracing::warn!(%uuid, error = %e, "kobo span→cfi task panicked");
            None
        }
    }
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
    let last_modified = match db::book_last_modified_for(state.pool(), id).await {
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
