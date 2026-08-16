//! Kobo library sync: `v1/library/sync` (the streamed per-device delta) and
//! the two per-book reads it shares logic with, `v1/library/{uuid}/metadata`
//! and the `state` GET half. Both read routes share [`enrich_states_with_derived_spans`]
//! so a CFI-only web position reaches the device as an exact KoboSpan.

use std::collections::HashMap;

use axum::{
    body::{Body, Bytes},
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use futures_util::stream;
use omnibus_db::{self as db, worker::Task};

use super::{dto, extractor::KoboAuthUser, origin_from_headers, reject_oversized_uuid, AppState};
use crate::http_errors::internal;

/// Changes per `library/sync` response; more remain → `x-kobo-sync: continue`, bounding the response but never the sync (unlike Calibre-Web's `SYNC_ITEM_LIMIT`).
const SYNC_PAGE_SIZE: usize = 100;

/// Per-sync cap on CFI→span derivations. Each one walks a whole book's
/// spine for the percent, so a page full of web-written positions must not
/// stall the sync; the clock-neutral write-back shrinks the candidate set
/// monotonically, so the remainder heals across subsequent syncs.
const SPAN_DERIVATIONS_PER_SYNC: usize = 8;

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
pub async fn library_sync(
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
    // buffer of the whole payload. Each poll dispatches to the step function
    // for the current `SyncPhase` (below); the dispatcher itself stays a
    // one-line match so the phase logic reads as ordinary functions rather
    // than a nested `move async` closure.
    let stream = stream::unfold(
        SyncStreamState {
            changes: delta.changes,
            base,
            token: auth.token,
            states,
            phase: SyncPhase::Open,
        },
        move |st| {
            let pool = pool.clone();
            async move {
                match st.phase {
                    SyncPhase::Open => Some(sync_stream_open(st)),
                    SyncPhase::Item { change, emitted } => {
                        Some(sync_stream_item(st, change, emitted))
                    }
                    SyncPhase::Close => Some(sync_stream_close(&pool, device_id, st).await),
                    SyncPhase::End => None,
                }
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

/// For page books whose freshest position is a web CFI (`kobo_location`
/// NULL, `epub_cfi` present), derive a KoboSpan + percent so the device
/// gets an exact position, and persist the derived halves clock-neutrally
/// so later syncs skip the work. Every failure leaves the state untouched —
/// the device then sees whatever percent exists, exactly as before.
async fn enrich_states_with_derived_spans(
    state: &AppState,
    user_id: i64,
    changes: &[db::kobo::SyncChange],
    states: &mut HashMap<String, db::kobo::KoboBookState>,
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
        // Follow-mode lane: when the audiobook holds the freshest position
        // on a follow link, serve the mapped spot instead of the reader's
        // older CFI. Response-only — the progress row keeps its real
        // provenance; the device's own PUT makes the position real once
        // the reader picks it up there.
        //
        // Cheap pre-filter before the multi-query mapping composition:
        // most changed books have no follow link, and one indexed
        // `get_link` read skips the timeline/progress/structure reads for
        // all of them (a large first sync touches every book). Falls
        // through to the normal derivation lane below either way.
        let follow_linked = matches!(
            db::cross_format::get_link(state.pool(), user_id, &book.uuid).await,
            Ok(Some(link)) if link.follow
        );
        let resume = if follow_linked {
            db::cross_format::resume_candidate(
                state.pool(),
                user_id,
                &book.uuid,
                omnibus_shared::ProgressFormat::Epub,
            )
            .await
            .ok()
        } else {
            None
        };
        if let Some(resume) = resume {
            if resume.follow
                && resume.state == omnibus_shared::cross_format::CrossFormatResumeState::Candidate
            {
                if let Some(c) = &resume.candidate {
                    if let Some(frac) = c.fraction {
                        budget -= 1;
                        let loc = derive_span_for_fraction(state, book.id, &book.uuid, frac).await;
                        if let Some(s) = states.get_mut(&book.uuid) {
                            if let Some(loc) = loc {
                                s.kobo_location = Some(loc);
                            }
                            s.percent = c.percent.or(s.percent);
                        }
                        continue;
                    }
                }
            }
        }
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

/// Best-effort fraction→KoboSpan derivation for the follow-mode lane:
/// spine stats invert the fraction to a `(spine, offset)` position, and
/// the kepub walk proves alignment before emitting a span. `None` serves
/// percent-only — truthful, just less precise — and queues the kepub
/// conversion for a later sync, mirroring [`derive_span_for_cfi`].
async fn derive_span_for_fraction(
    state: &AppState,
    book_id: i64,
    uuid: &str,
    frac: f64,
) -> Option<String> {
    let (file_id, source) = db::book_file_with_id(state.pool(), book_id, "EPUB")
        .await
        .ok()??;
    let stats = db::epub_structure::get_spine_stats(state.pool(), file_id)
        .await
        .ok()?;
    let (spine_index, offset) = db::epub_structure::position_at_fraction(&stats, frac)?;
    let kepub = db::kepub_path(book_id);
    if !tokio::fs::try_exists(&kepub).await.unwrap_or(false) {
        state.worker().post(Task::KepubConvert { book_id });
        return None;
    }
    let uuid = uuid.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        db::kobo_position::span_at_spine_offset(&kepub, &source, spine_index as usize, offset)
    })
    .await;
    match result {
        Ok(Ok(loc)) => loc,
        Ok(Err(e)) => {
            tracing::warn!(%uuid, error = %e, "kobo fraction→span derivation failed");
            None
        }
        Err(e) => {
            tracing::warn!(%uuid, error = %e, "kobo fraction→span task panicked");
            None
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

/// Accumulator threaded through [`library_sync`]'s `stream::unfold` — one
/// field per piece the state used to carry as an anonymous tuple, named so
/// each step function below reads without positional guessing.
struct SyncStreamState {
    changes: Vec<db::kobo::SyncChange>,
    base: String,
    token: String,
    states: HashMap<String, db::kobo::KoboBookState>,
    phase: SyncPhase,
}

/// Wrap a chunk as the infallible `Result` item `Body::from_stream` expects.
fn ok(bytes: Bytes) -> Result<Bytes, std::io::Error> {
    Ok(bytes)
}

/// `SyncPhase::Open` step: emit the opening `[` and advance to the first
/// item, or straight to `Close` for an empty delta.
fn sync_stream_open(mut st: SyncStreamState) -> (Result<Bytes, std::io::Error>, SyncStreamState) {
    st.phase = if st.changes.is_empty() {
        SyncPhase::Close
    } else {
        SyncPhase::Item {
            change: 0,
            emitted: 0,
        }
    };
    (ok(Bytes::from_static(b"[")), st)
}

/// `SyncPhase::Item` step: serialize the current change's DTOs (a change can
/// fan out into two items) and advance to the next change or `Close`. A JSON
/// serialization failure — never expected, since every DTO is owned
/// Strings/primitives, but handled rather than unwrapped — aborts the body
/// via `SyncPhase::End` instead of emitting malformed JSON: `record_synced`
/// then never runs, and the device retries the same delta next sync.
fn sync_stream_item(
    mut st: SyncStreamState,
    change: usize,
    emitted: usize,
) -> (Result<Bytes, std::io::Error>, SyncStreamState) {
    let uuid = match &st.changes[change] {
        db::kobo::SyncChange::New(b)
        | db::kobo::SyncChange::Changed(b)
        | db::kobo::SyncChange::StateChanged(b) => Some(b.uuid.as_str()),
        db::kobo::SyncChange::Removed { .. } => None,
    };
    let book_state = uuid.and_then(|u| st.states.get(u));
    let items = dto::sync_items(&st.base, &st.token, &st.changes[change], book_state);
    let mut piece = Vec::new();
    for item in &items {
        let json = match serde_json::to_vec(item) {
            Ok(j) => j,
            Err(e) => {
                st.phase = SyncPhase::End;
                return (Err(std::io::Error::other(e)), st);
            }
        };
        if emitted > 0 || !piece.is_empty() {
            piece.push(b',');
        }
        piece.extend_from_slice(&json);
    }
    st.phase = if change + 1 < st.changes.len() {
        SyncPhase::Item {
            change: change + 1,
            emitted: emitted + items.len(),
        }
    } else {
        SyncPhase::Close
    };
    (ok(Bytes::from(piece)), st)
}

/// `SyncPhase::Close` step: the body is complete, so commit the device's
/// sync snapshot. A failure here is deliberately non-fatal to the response
/// (the bytes are already on the wire) — the device just replays the same
/// delta next sync, which is safe.
async fn sync_stream_close(
    pool: &sqlx::SqlitePool,
    device_id: i64,
    mut st: SyncStreamState,
) -> (Result<Bytes, std::io::Error>, SyncStreamState) {
    if let Err(e) = db::kobo::record_synced(pool, device_id, &st.changes).await {
        tracing::warn!(device_id, error = %e, "kobo snapshot advance failed");
    }
    st.phase = SyncPhase::End;
    (ok(Bytes::from_static(b"]")), st)
}

/// `GET library/<uuid>/metadata` — the single-book metadata array Kobo fetches
/// before downloading.
pub async fn library_metadata(
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
pub async fn get_state(
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
