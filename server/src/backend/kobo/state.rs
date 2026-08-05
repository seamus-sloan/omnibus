//! Kobo reading-state write path: `PUT v1/library/{uuid}/state`. Splits a
//! device's batch into read-status writes (`book_read_status`) and bookmark
//! plus statistics writes (`reading_progress`), sharing one transaction
//! across the batch.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use omnibus_shared::{ProgressFormat, ProgressUpdate, ReadStatus, SetReadStatus};
use sqlx::{Sqlite, Transaction};

use super::{dto, extractor::KoboAuthUser, reject_oversized_uuid, AppState};
use crate::http_errors::internal;

/// Cap a `state` PUT body at [`dto::StateRequest::MAX_READING_STATES`] entries
/// before any DB work, mirroring [`super::reject_oversized_uuid`].
/// `Some(response)` is the rejection.
fn reject_oversized_state_request(body: &dto::StateRequest) -> Option<Response> {
    (body.reading_states.len() > dto::StateRequest::MAX_READING_STATES)
        .then(|| StatusCode::BAD_REQUEST.into_response())
}

/// `PUT library/<uuid>/state` — persist the device's reading state.
/// `StatusInfo` routes into `book_read_status`; the `CurrentBookmark`
/// position lands in `reading_progress` as percent + verbatim `KoboSpan`
/// plus a server-derived `epub_cfi` when the cached KEPUB allows it
/// (#925), stamped with the device's own event time when it sends one. A
/// `Statistics` block is mirrored onto the same row so sync-out can echo it
/// back unchanged (#1653) — it is never aggregated into `db::stats`.
///
/// The whole batch's read-status and bookmark writes share one transaction
/// (begun before the loop, committed after it), so a mid-batch DB failure
/// rolls back every entry rather than leaving the sync half-applied — a
/// `BookNotFound` for one entry is not such a failure and is logged and
/// skipped within the same transaction.
pub async fn put_state(
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
        if let Some(stats) = &entry.statistics {
            // Kept from #1652's capture pass — the shape came from the
            // reference impls, not from hardware.
            tracing::info!(
                %uuid,
                spent_reading_minutes = ?stats.spent_reading_minutes,
                remaining_time_minutes = ?stats.remaining_time_minutes,
                last_modified = ?stats.last_modified,
                "kobo statistics received"
            );
            // Same clock ladder as the bookmark, most-specific first.
            let event_ts = stats
                .last_modified
                .as_deref()
                .or(entry.last_modified.as_deref())
                .or(entry.priority_timestamp.as_deref())
                .and_then(dto::parse_kobo_timestamp);
            match persist_statistics(&mut tx, auth.user_id, &uuid, stats, event_ts).await {
                Ok(()) => {}
                Err(db::progress::ProgressError::BookNotFound) => {
                    tracing::warn!(%uuid, "kobo statistics PUT for unknown book");
                }
                Err(db::progress::ProgressError::Sqlx(e)) => {
                    return internal("kobo set_kobo_statistics", e);
                }
            }
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

/// Mirror a device's `Statistics` block so sync-out can hand the same numbers
/// back with the device's own clock (#1653).
///
/// A negative counter is dropped, not clamped, like an out-of-range percent:
/// echoed-only values have nothing safe to clamp toward, and the row CHECK
/// would abort the whole batch rather than this one entry.
async fn persist_statistics(
    tx: &mut Transaction<'_, Sqlite>,
    user_id: i64,
    uuid: &str,
    stats: &dto::Statistics,
    event_ts: Option<i64>,
) -> Result<(), db::progress::ProgressError> {
    let update = db::progress::KoboStatistics {
        spent_reading_minutes: stats.spent_reading_minutes.filter(|m| *m >= 0),
        remaining_time_minutes: stats.remaining_time_minutes.filter(|m| *m >= 0),
        updated_at: event_ts,
    };
    if update.is_empty() {
        tracing::debug!(%uuid, "kobo statistics carried no usable counter");
        return Ok(());
    }
    if !db::progress::set_kobo_statistics_tx(tx, user_id, uuid, &update).await? {
        // Either the book has no epub position row yet (statistics annotate a
        // position, they don't create one) or a newer block is already stored.
        tracing::debug!(%uuid, "kobo statistics not stored");
    }
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

/// Map a Kobo `StatusInfo.Status` token to the internal read status. Unknown
/// tokens (and `ReadyToRead`) fall back to `Unread`.
fn map_status(kobo: &str) -> ReadStatus {
    match kobo {
        "Finished" => ReadStatus::Finished,
        "Reading" => ReadStatus::Reading,
        _ => ReadStatus::Unread,
    }
}
