//! Kobo per-book resources: KEPUB download, cover thumbnails, and the
//! (currently empty) tags collection. `download` and `image` both resolve
//! the book id from the path uuid before touching the filesystem/DB.

use axum::{
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    self as db,
    worker::{Task, TaskOutcome},
};

use super::{extractor::KoboAuthUser, reject_oversized_uuid, AppState};
use crate::backend::serve_download;
use crate::http_errors::internal;

/// Wall-clock budget for an inline KEPUB conversion on a Kobo download before
/// falling back to plain EPUB. Mirrors the USB sideload path's budget.
const KEPUB_CONVERT_BUDGET: std::time::Duration = std::time::Duration::from_secs(25);

/// `GET library/tags` — device-side collections. Slice A returns an empty set;
/// shelves-as-collections is #924.
pub async fn library_tags(_auth: KoboAuthUser) -> Response {
    Json(serde_json::json!([])).into_response()
}

/// `GET download/<uuid>` — serve the book as KEPUB (converting via the worker,
/// cached), falling back to plain EPUB when kepubify is absent, conversion
/// fails, or it exceeds [`KEPUB_CONVERT_BUDGET`]. Streamed with range support.
pub async fn download(
    auth: KoboAuthUser,
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
        crate::backend::ebooks::rewritten_or_source(&state, id, source).await
    };
    let response = serve_download(req, &path, "application/epub+zip").await;
    // Only bookkeep on an answer that means the device actually has (or
    // already had, per a conditional 304) the bytes — #1647's bug was
    // exactly this call running unconditionally, so a 404 (bad path, file
    // open failure) or 412/416 still marked the device as holding a book it
    // never received. Checked *after* `serve_download` returns, against its
    // real status, rather than inferred from path resolution succeeding —
    // path resolution success does not guarantee `conditional::open` does.
    if response.status().is_success() || response.status() == StatusCode::NOT_MODIFIED {
        if let Err(e) = record_download_state(&state, &auth, &uuid).await {
            tracing::warn!(error = %e, "kobo download-state record failed");
        }
    }
    response
}

/// Record that `auth`'s device now holds this book (#1647) — the gate
/// `ack_served` checks before letting a later `GET .../annotations` advance
/// its watermark — and give any web-origin annotation still waiting on a
/// KEPUB cache one more chance to downsync. Only called once the caller has
/// confirmed the response actually delivered (or already held) the file;
/// `downsync_book_annotations` itself still derives nothing when the KEPUB
/// cache (or the source EPUB) isn't on disk, which is routine in the
/// plain-EPUB fallback path — this call still records the device's download
/// in that case, and the worker handle leaves a conversion queued so the
/// derivation lands later instead of never. Awaited by the caller, so it adds
/// latency to the response; kept best-effort and non-fatal regardless, since
/// annotation bookkeeping must never turn into an error the device retries.
async fn record_download_state(
    state: &AppState,
    auth: &KoboAuthUser,
    uuid: &str,
) -> anyhow::Result<()> {
    let Some(canonical) = db::resolve_canonical_book_uuid(state.pool(), uuid).await? else {
        return Ok(());
    };
    db::kobo::annotations::mark_downloaded(state.pool(), auth.device_id, &canonical).await?;
    db::annotations::downsync_book_annotations(
        state.pool(),
        Some(state.worker()),
        auth.user_id,
        &canonical,
    )
    .await?;
    Ok(())
}

/// `GET books/<uuid>/thumbnail/.../image.jpg` — serve the book cover. The
/// requested dimensions are advisory; the stored cover is served as-is.
///
/// Carries a weak `ETag` derived from `(book id, last_modified)` and honors
/// `If-None-Match` with a bodyless 304 — the device re-validates covers on
/// every sync, so without this each sync re-downloads every cover it already
/// holds.
pub async fn image(
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
