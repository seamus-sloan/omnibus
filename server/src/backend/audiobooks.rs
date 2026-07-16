//! Audiobook streaming routes: direct-play manifest (`/manifest`) + Range-
//! served part files (`/parts/{ordinal}`), with the legacy HLS pipeline
//! (`/playlist.m3u8`, `/segments/{segment}`, `/status`) retained as a
//! fallback for non-natively-playable codecs. See
//! [`get_audiobook_manifest`] for the direct-vs-HLS routing rule.

use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    http::HeaderValue,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    audiobook::{self, PlaybackMode},
    hls,
    progress::ProgressError,
    worker::Task,
};
use omnibus_shared::{AudiobookManifest, AudiobookPlaybackRateUpdate, ManifestPart};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{internal, AppState};
use crate::auth::{AuthUser, MediaAuthUser};

/// Content-Type for MPEG-TS audio segments served by the HLS fallback.
/// Defined at module scope so the per-request `HeaderValue` insert is a
/// cheap clone of a compile-time-validated static instead of a `.parse()`
/// + `.expect()` on every segment fetch.
static MPEGTS_CONTENT_TYPE: HeaderValue = HeaderValue::from_static("video/MP2T");

/// Query parameters for `GET /api/audiobooks/{uuid}/manifest`.
#[derive(Deserialize)]
pub(super) struct ManifestQuery {
    file_id: Option<i64>,
}

/// Return the authenticated user's saved playback rate for an audiobook.
pub(super) async fn get_playback_rate(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match omnibus_db::progress::get_playback_rate(&state.pool, user.id, &uuid).await {
        Ok(record) => Json(record).into_response(),
        Err(ProgressError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ProgressError::Sqlx(e)) => internal("get_playback_rate", e),
    }
}

/// Persist the authenticated user's playback rate for an audiobook.
pub(super) async fn put_playback_rate(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(update): Json<AudiobookPlaybackRateUpdate>,
) -> Response {
    if let Err(message) = update.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, message).into_response();
    }
    match omnibus_db::progress::set_playback_rate(&state.pool, user.id, &uuid, &update).await {
        Ok(record) => Json(record).into_response(),
        Err(ProgressError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ProgressError::Sqlx(e)) => internal("set_playback_rate", e),
    }
}

/// Returns the HLS VOD manifest for `uuid` (always built from DB — no
/// filesystem read). The manifest is derived from stored
/// `book_file_parts.duration_seconds` values so it stays accurate before
/// and after transcoding.
pub(super) async fn get_audiobook_playlist(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let resolved = match hls::resolve_audiobook(&state.pool, &uuid).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook", e),
    };

    // Once the transcode is complete, ffmpeg's own `index.m3u8` is the
    // source of truth — its segment count matches what's actually on
    // disk, which the DB-built manifest (rounded from
    // `book_file_parts.duration_seconds`) can miss by ±1 because
    // `-hls_time 10` is advisory (keyframe / packet boundaries). Falling
    // back to the DB-built stub when the file is missing (eviction race,
    // permissions) keeps the manifest available pre-transcode too.
    let m3u8 = if hls::is_ready(resolved.book_id, hls::AUDIO64) {
        match hls::read_ffmpeg_manifest(resolved.book_id, hls::AUDIO64) {
            Some(s) => s,
            None => {
                let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
                    Ok(p) => p,
                    Err(e) => return internal("get_parts", e),
                };
                hls::build_manifest(&parts)
            }
        }
    } else {
        let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
            Ok(p) => p,
            Err(e) => return internal("get_parts", e),
        };
        hls::build_manifest(&parts)
    };
    (
        axum::http::StatusCode::OK,
        [(
            axum::http::header::CONTENT_TYPE,
            "application/vnd.apple.mpegurl",
        )],
        m3u8,
    )
        .into_response()
}

/// Returns the playback manifest for `uuid`. Accepts an optional
/// `?file_id=N` to target a specific `book_files` row (for multi-file
/// books after merge). Routes direct-playable books (m4b / m4a / mp3) to
/// per-part HTTP Range URLs and everything else to the legacy HLS playlist.
///
/// See [`omnibus_db::audiobook::codec`] for the direct-vs-HLS gate; note
/// that [`omnibus_db::hls::resolve_audiobook`] only admits books whose
/// top-level `book_files.format` is one of `M4B` / `M4A` / `MP3`, so
/// pure-AAC or pure-FLAC sources never reach direct mode today.
pub(super) async fn get_audiobook_manifest(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(query): Query<ManifestQuery>,
) -> Response {
    let resolved = match hls::resolve_audiobook_file(&state.pool, &uuid, query.file_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook", e),
    };
    let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
        Ok(p) => p,
        Err(e) => return internal("get_parts", e),
    };
    if parts.is_empty() {
        // Indexer somehow produced a book_files row with zero parts —
        // either a migration left the audiobook in a half-state or a
        // mid-flight reindex was interrupted. Don't speculate; 404 so
        // the user clicks Reindex.
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }

    let filenames: Vec<&str> = parts.iter().map(|p| p.filename.as_str()).collect();
    let file_id_suffix = query
        .file_id
        .map(|fid| format!("?file_id={fid}"))
        .unwrap_or_default();
    let manifest = match audiobook::classify_filenames(&filenames) {
        PlaybackMode::Direct => {
            let total_duration_seconds = parts.iter().map(|p| p.duration_seconds).sum();
            let manifest_parts = parts
                .iter()
                .map(|p| ManifestPart {
                    ordinal: p.ordinal,
                    url: format!(
                        "/api/audiobooks/{uuid}/parts/{}{}",
                        p.ordinal, file_id_suffix
                    ),
                    duration_seconds: p.duration_seconds,
                    mime: audiobook::mime_for_filename(&p.filename).to_string(),
                })
                .collect();
            let chapters = match hls::get_chapters(&state.pool, resolved.book_file_id).await {
                Ok(c) => c,
                Err(e) => return internal("get_chapters", e),
            };
            AudiobookManifest::Direct {
                parts: manifest_parts,
                total_duration_seconds,
                chapters,
            }
        }
        PlaybackMode::Hls => AudiobookManifest::Hls {
            playlist_url: format!("/api/audiobooks/{uuid}/playlist.m3u8"),
        },
    };

    Json(manifest).into_response()
}

/// Query parameters for `GET /api/audiobooks/{uuid}/parts/{ordinal}`.
#[derive(Deserialize)]
pub(super) struct PartsQuery {
    file_id: Option<i64>,
}

/// Range-served raw file for one part of a direct-play audiobook.
/// Accepts optional `?file_id=N` to target a specific `book_files` row.
///
/// Authenticated via [`MediaAuthUser`] (not [`AuthUser`]) because this URL is
/// wired straight into the mobile WebView's `<audio src>`, whose fetch can
/// carry neither the native `reqwest` bearer header nor a session cookie — the
/// `?token=` query param is the only auth it has. Kept in lockstep with
/// `is_media_read_path` in `auth::gate`.
pub(super) async fn get_audiobook_part(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path((uuid, ordinal)): Path<(String, i64)>,
    Query(query): Query<PartsQuery>,
    req: Request,
) -> Response {
    let resolved = match hls::resolve_audiobook_file(&state.pool, &uuid, query.file_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook", e),
    };
    let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
        Ok(p) => p,
        Err(e) => return internal("get_parts", e),
    };
    // Mirror the manifest classifier: refuse to serve source files for
    // a book that `/manifest` would have routed through HLS. Keeps the
    // API contract honest — clients should never reach this endpoint
    // for an HLS-mode book.
    let filenames: Vec<&str> = parts.iter().map(|p| p.filename.as_str()).collect();
    if audiobook::classify_filenames(&filenames) != PlaybackMode::Direct {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    }
    let Some(part) = parts.into_iter().find(|p| p.ordinal == ordinal) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };

    let abs_path = std::path::Path::new(&resolved.library_path).join(&part.filename);
    let mime = audiobook::mime_for_filename(&part.filename);
    let serve = ServeFile::new(&abs_path);
    let res = match serve.oneshot(req).await {
        Ok(r) => r,
        Err(e) => return internal("serve audiobook part", e),
    };
    let (mut parts_resp, body) = res.into_parts();
    if let Ok(value) = mime.parse() {
        parts_resp
            .headers
            .insert(axum::http::header::CONTENT_TYPE, value);
    }
    Response::from_parts(parts_resp, Body::new(body))
}

/// Query parameters for `GET /api/audiobooks/{uuid}/download`.
#[derive(Deserialize)]
pub(super) struct DownloadQuery {
    file_id: Option<i64>,
}

/// Serves the audiobook's source file as a browser download
/// (`Content-Disposition: attachment`), streaming via `ServeFile`.
///
/// Targets the lowest-ordinal part of the resolved `book_files` row
/// (optionally selected via `?file_id=N`). For a single-file audiobook
/// (the common m4b case) that is the whole book; multi-part sources only
/// yield the first part today — there is no on-the-fly archiving yet, so
/// per-part downloads go through the format switcher's file picker.
pub(super) async fn get_audiobook_download(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(query): Query<DownloadQuery>,
    req: Request,
) -> Response {
    let resolved = match hls::resolve_audiobook_file(&state.pool, &uuid, query.file_id).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook_file", e),
    };
    let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
        Ok(p) => p,
        Err(e) => return internal("get_parts", e),
    };
    let Some(part) = parts.into_iter().min_by_key(|p| p.ordinal) else {
        return axum::http::StatusCode::NOT_FOUND.into_response();
    };
    let abs_path = std::path::Path::new(&resolved.library_path).join(&part.filename);
    let mime = audiobook::mime_for_filename(&part.filename);
    super::serve_download(req, &abs_path, mime).await
}

/// Returns `{"ready": bool, "progress": f32}` for the AUDIO64 profile.
/// If the transcode has not started yet, fires a `Task::HlsTranscode`
/// fire-and-forget to the worker.
pub(super) async fn get_audiobook_status(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let resolved = match hls::resolve_audiobook(&state.pool, &uuid).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook", e),
    };

    let book_id = resolved.book_id;
    let ready = hls::is_ready(book_id, hls::AUDIO64);
    let failed = hls::has_failed(book_id, hls::AUDIO64);
    let progress = hls::read_progress(book_id, hls::AUDIO64);

    // If neither ready nor in progress, kick off the transcode. The worker
    // serializes duplicate posts via the per-resource keyed mutex so a
    // second poll while the job is already running just enqueues behind
    // it. Skip the kick entirely when a `.failed` marker is present —
    // otherwise the frontend's 1 s status poll becomes an unbounded retry
    // loop on a permanently-broken book (corrupt source, missing ffmpeg).
    if !ready && progress < 0.05 && !failed {
        state.worker.post(Task::HlsTranscode {
            book_id,
            book_file_id: resolved.book_file_id,
            library_path: resolved.library_path,
            profile: hls::AUDIO64.to_string(),
        });
    }

    // Discriminator the frontend uses to render preparing / ready / failed
    // states without ambiguity. Fixes Bug 4 from #338 — the old shape
    // (`ready: false, progress: 0`) was indistinguishable from
    // "permanently failed" so the UI stuck on "Preparing" forever.
    let status_state = if ready {
        StatusState::Ready
    } else if failed {
        StatusState::Failed
    } else {
        StatusState::Preparing
    };

    Json(StatusResponse {
        ready,
        progress,
        state: status_state,
    })
    .into_response()
}

/// Serves one `.ts` segment file from the HLS cache.
///
/// If the segment is absent (transcode not yet complete) the handler
/// posts a `Task::HlsTranscode` and blocks until it finishes. After
/// completion it re-checks for the file and serves it, or 404s if
/// ffmpeg failed to produce it.
pub(super) async fn get_audiobook_segment(
    _user: AuthUser,
    State(state): State<AppState>,
    Path((uuid, segment)): Path<(String, String)>,
    req: Request,
) -> Response {
    // Validate: segment must match `seg-NNNN.ts` exactly.
    if !is_valid_segment_name(&segment) {
        return axum::http::StatusCode::BAD_REQUEST.into_response();
    }

    let resolved = match hls::resolve_audiobook(&state.pool, &uuid).await {
        Ok(Some(r)) => r,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_audiobook", e),
    };

    let book_id = resolved.book_id;
    let seg_path = hls::segment_dir(book_id, hls::AUDIO64).join(&segment);

    // Fast path: segment already on disk.
    if !seg_path.exists() {
        // Permanent failure short-circuits the kick: a previous transcode
        // already cleaned the dir + wrote `.failed`, and any retry would
        // immediately bail with the same error. Return 503 so hls.js
        // surfaces a real failure to the user instead of stalling on a
        // never-arriving segment.
        if hls::has_failed(book_id, hls::AUDIO64) {
            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        // Transcode hasn't produced this segment yet — trigger and wait.
        let task_id = state.worker.post(Task::HlsTranscode {
            book_id,
            book_file_id: resolved.book_file_id,
            library_path: resolved.library_path,
            profile: hls::AUDIO64.to_string(),
        });
        let _ = state.worker.await_completion(task_id).await;

        if !seg_path.exists() {
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    }

    let serve = ServeFile::new(&seg_path);
    let res = match serve.oneshot(req).await {
        Ok(r) => r,
        Err(e) => return internal("serve segment", e),
    };
    let (mut parts, body) = res.into_parts();
    parts.headers.insert(
        axum::http::header::CONTENT_TYPE,
        MPEGTS_CONTENT_TYPE.clone(),
    );
    Response::from_parts(parts, Body::new(body))
}

/// Terminal-vs-in-flight discriminator for the status JSON. Lets the
/// frontend distinguish "transcode is preparing" from "transcode
/// permanently failed" — the legacy `{ready: false, progress: 0}` shape
/// collapsed both into the same ambiguous response.
#[derive(Serialize)]
#[serde(rename_all = "lowercase")]
enum StatusState {
    /// Transcode is queued or running.
    Preparing,
    /// Transcode finished — segments + manifest are on disk.
    Ready,
    /// Transcode terminally failed (`.failed` marker present). The UI
    /// renders an error with a retry affordance.
    Failed,
}

/// JSON response body for `GET /api/audiobooks/{uuid}/status`.
///
/// `ready` and `progress` are kept for compatibility with any client
/// that pre-dates the `state` field; new clients should branch on
/// `state` only.
#[derive(Serialize)]
struct StatusResponse {
    ready: bool,
    progress: f32,
    state: StatusState,
}

/// `true` if `name` matches `seg-NNNN.ts` exactly (4 decimal digits).
///
/// Case-insensitive on prefix and extension so the validator stays in
/// sync with case-insensitive filesystems (APFS, NTFS) — the transcoder
/// writes lowercase today, but `seg-0001.TS` from the same file must
/// still resolve. Compares byte slices via `eq_ignore_ascii_case` to
/// avoid allocating a lowercased copy on the HLS segment hot path.
fn is_valid_segment_name(name: &str) -> bool {
    if name.len() != 11 {
        return false;
    }
    let bytes = name.as_bytes();
    bytes[..4].eq_ignore_ascii_case(b"seg-")
        && bytes[8..].eq_ignore_ascii_case(b".ts")
        && bytes[4..8].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests;
