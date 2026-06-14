//! Audiobook streaming routes: direct-play manifest + Range-served part
//! files, with the legacy HLS pipeline retained as a fallback for
//! non-natively-playable codecs.
//!
//! - `GET /api/audiobooks/{uuid}/manifest` — JSON describing how the
//!   frontend should play this book: `direct` (per-part URLs the client
//!   chains together) for m4b / m4a / mp3, or `hls` (playlist URL) for
//!   anything else. See [`omnibus_db::audiobook::codec`] for the gate;
//!   note that [`omnibus_db::hls::resolve_audiobook`] only admits books
//!   whose top-level `book_files.format` is one of `M4B` / `M4A` / `MP3`,
//!   so pure-AAC or pure-FLAC sources never reach this handler today.
//! - `GET /api/audiobooks/{uuid}/parts/{ordinal}` — Range-served source
//!   file for one part of a direct-play book.
//! - `GET /api/audiobooks/{uuid}/playlist.m3u8` — fallback HLS manifest
//!   built from `book_file_parts.duration_seconds`; only reachable when
//!   the manifest endpoint returns `mode: hls`.
//! - `GET /api/audiobooks/{uuid}/segments/{segment}` — fallback HLS
//!   segment serve from the transcode cache.
//! - `GET /api/audiobooks/{uuid}/status` — fallback transcode-readiness
//!   poll (`{"ready":bool,"progress":f32}`) for the HLS path.

use axum::{
    body::Body,
    extract::{Path, Query, Request, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    audiobook::{self, PlaybackMode},
    hls,
    worker::Task,
};
use omnibus_shared::{AudiobookManifest, ManifestPart};
use serde::{Deserialize, Serialize};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{internal, AppState};
use crate::auth::AuthUser;

/// Query parameters for `GET /api/audiobooks/{uuid}/manifest`.
#[derive(Deserialize)]
pub(super) struct ManifestQuery {
    file_id: Option<i64>,
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
pub(super) async fn get_audiobook_part(
    _user: AuthUser,
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
        "video/MP2T".parse().expect("static content-type"),
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
/// still resolve. Clippy's
/// `case_sensitive_file_extension_comparisons` lint is silenced because
/// the `to_ascii_lowercase` pass below already makes the `.ends_with`
/// comparison case-insensitive without reaching for `Path::extension`.
#[allow(clippy::case_sensitive_file_extension_comparisons)]
fn is_valid_segment_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if !lower.starts_with("seg-") || !lower.ends_with(".ts") {
        return false;
    }
    let digits = &lower[4..lower.len() - 3];
    digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests;
