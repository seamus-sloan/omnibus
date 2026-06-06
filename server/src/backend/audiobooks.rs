//! F2.3 audiobook streaming — direct-play manifest (#339) + Range-served
//! part files, with the legacy HLS pipeline retained as a fallback for
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
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    audiobook::{self, PlaybackMode},
    hls,
    worker::Task,
};
use omnibus_shared::{AudiobookManifest, ManifestPart};
use serde::Serialize;
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::{internal, AppState};
use crate::auth::AuthUser;

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

/// Returns the playback manifest for `uuid`. Routes direct-playable
/// books (m4b / m4a / mp3) to per-part HTTP Range URLs and everything
/// else (mixed folders containing flac / ac3 / …) to the legacy HLS
/// playlist. The codec gate lives in [`omnibus_db::audiobook::codec`];
/// see [#339](https://github.com/seamus-sloan/omnibus/issues/339) for
/// the design.
pub(super) async fn get_audiobook_manifest(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    let resolved = match hls::resolve_audiobook(&state.pool, &uuid).await {
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
    let manifest = match audiobook::classify_filenames(&filenames) {
        PlaybackMode::Direct => {
            let total_duration_seconds = parts.iter().map(|p| p.duration_seconds).sum();
            let manifest_parts = parts
                .iter()
                .map(|p| ManifestPart {
                    ordinal: p.ordinal,
                    url: format!("/api/audiobooks/{uuid}/parts/{}", p.ordinal),
                    duration_seconds: p.duration_seconds,
                    mime: audiobook::mime_for_filename(&p.filename).to_string(),
                })
                .collect();
            AudiobookManifest::Direct {
                parts: manifest_parts,
                total_duration_seconds,
            }
        }
        PlaybackMode::Hls => AudiobookManifest::Hls {
            playlist_url: format!("/api/audiobooks/{uuid}/playlist.m3u8"),
        },
    };

    Json(manifest).into_response()
}

/// Range-served raw file for one part of a direct-play audiobook.
/// Mirrors the `get_audiobook_segment` HLS-segment path (same
/// `ServeFile` + `oneshot` shape) but reads the source file from the
/// library rather than the HLS cache. Out-of-range ordinal → 404;
/// missing file on disk → 404 from `ServeFile` itself. HLS-classified
/// books (e.g. mixed folders containing flac) → 404, since `/manifest`
/// already routed them to the HLS playlist.
pub(super) async fn get_audiobook_part(
    _user: AuthUser,
    State(state): State<AppState>,
    Path((uuid, ordinal)): Path<(String, i64)>,
    req: Request,
) -> Response {
    let resolved = match hls::resolve_audiobook(&state.pool, &uuid).await {
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
/// permanently failed" — the legacy `{ready: false, progress: 0}`
/// shape collapsed both into the same response, which is Bug 4 of #338.
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
fn is_valid_segment_name(name: &str) -> bool {
    if !name.starts_with("seg-") || !name.ends_with(".ts") {
        return false;
    }
    let digits = &name[4..name.len() - 3];
    digits.len() == 4 && digits.chars().all(|c| c.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    /// Serializes the status-endpoint tests that mutate `OMNIBUS_DATA_DIR`.
    /// `hls::has_failed` reads the env var on every call, so two tests
    /// pointing at different tempdirs will race and one will see the
    /// other's `.failed` marker. Mirrors the `ENV_LOCK` pattern in
    /// `db/src/thumbs.rs`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Seed one audiobook book + book_files + book_file_parts row for tests.
    async fn seed_one_audiobook(pool: &sqlx::SqlitePool) -> String {
        let lib_id =
            sqlx::query("INSERT INTO libraries (path, display_name) VALUES (?, 'audiobooks')")
                .bind("/audiobooks")
                .execute(pool)
                .await
                .unwrap()
                .last_insert_rowid();
        let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
        let book_id =
            sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'PK')")
                .bind(uuid)
                .bind(lib_id)
                .bind("/audiobooks")
                .execute(pool)
                .await
                .unwrap()
                .last_insert_rowid();
        let file_id = sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
             VALUES (?, 'MP3', 'the-princess-knight', 100, '')",
        )
        .bind(book_id)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query(
            "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES (?, 0, 'ch01.mp3', 50, 0, 300.0)",
        )
        .bind(file_id)
        .execute(pool)
        .await
        .unwrap();
        uuid.to_string()
    }

    #[tokio::test]
    async fn api_get_audiobook_playlist_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/playlist.m3u8"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_audiobook_playlist_returns_404_for_unknown_uuid() {
        let (app, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/does-not-exist/playlist.m3u8",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_audiobook_playlist_returns_m3u8_for_known_uuid() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_one_audiobook(&pool).await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/playlist.m3u8"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "application/vnd.apple.mpegurl");
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let body_str = std::str::from_utf8(&body).unwrap();
        assert!(
            body_str.contains("#EXTM3U"),
            "manifest should contain #EXTM3U"
        );
        assert!(
            body_str.contains("seg-0000.ts"),
            "manifest should reference seg-0000.ts"
        );
    }

    #[tokio::test]
    async fn api_get_audiobook_status_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/status"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_audiobook_status_returns_404_for_unknown_uuid() {
        let (app, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/does-not-exist/status",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    // Std mutex held across awaits is the intent — env vars are
    // process-global and we serialize sibling tests that mutate
    // `OMNIBUS_DATA_DIR`. Safe under tokio's current-thread test runtime.
    #[allow(clippy::await_holding_lock)]
    async fn api_get_audiobook_status_returns_preparing_when_not_transcoded() {
        // Hold the env lock for the whole test so the `failed_marker`
        // sibling test below can't interleave its `.failed` write into
        // our `OMNIBUS_DATA_DIR`. Tempdir keeps us isolated from any
        // pre-existing `./data/hls/*/audio64.failed` files on the host.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("OMNIBUS_DATA_DIR").ok();
        // SAFETY: held under ENV_LOCK; no other thread mutates the env.
        unsafe {
            std::env::set_var("OMNIBUS_DATA_DIR", dir.path());
        }

        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_one_audiobook(&pool).await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/status"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["ready"], serde_json::Value::Bool(false));
        // New `state` field (#339 / Bug 4 of #338) — lets the UI
        // distinguish "preparing" from "failed".
        assert_eq!(json["state"], "preparing");

        unsafe {
            match prev {
                Some(v) => std::env::set_var("OMNIBUS_DATA_DIR", v),
                None => std::env::remove_var("OMNIBUS_DATA_DIR"),
            }
        }
    }

    #[tokio::test]
    #[allow(clippy::await_holding_lock)] // see sibling test for rationale
    async fn api_get_audiobook_status_returns_failed_when_failed_marker_present() {
        // Direct fs poke: write the `.failed` marker that
        // `cleanup_segment_dir` writes on a terminal ffmpeg failure, then
        // assert the status endpoint surfaces `state: "failed"` instead of
        // the legacy `ready:false, progress:0` shape that the UI couldn't
        // distinguish from "preparing".
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let prev = std::env::var("OMNIBUS_DATA_DIR").ok();
        // SAFETY: held under ENV_LOCK; no other thread mutates the env.
        unsafe {
            std::env::set_var("OMNIBUS_DATA_DIR", dir.path());
        }

        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_one_audiobook(&pool).await;
        let book_id = sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
            .bind(&uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
        let book_dir = dir.path().join("hls").join(book_id.to_string());
        std::fs::create_dir_all(&book_dir).unwrap();
        std::fs::write(book_dir.join("audio64.failed"), "").unwrap();

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/status"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["state"], "failed");

        unsafe {
            match prev {
                Some(v) => std::env::set_var("OMNIBUS_DATA_DIR", v),
                None => std::env::remove_var("OMNIBUS_DATA_DIR"),
            }
        }
    }

    /// Seed an audiobook with N custom parts. Used by the manifest tests
    /// to exercise direct, hls, and per-ordinal-lookup code paths from a
    /// single helper. `library_path` becomes the prefix that
    /// `get_audiobook_part` joins to each part's filename.
    async fn seed_audiobook_with_parts(
        pool: &sqlx::SqlitePool,
        library_path: &str,
        format: &str,
        parts: &[(i64, &str, f64)],
    ) -> String {
        let lib_id = sqlx::query("INSERT INTO libraries (path, display_name) VALUES (?, 'lib')")
            .bind(library_path)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let uuid = format!("uuid-{}-{}", format.to_lowercase(), parts.len());
        let book_id =
            sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'T')")
                .bind(&uuid)
                .bind(lib_id)
                .bind(library_path)
                .execute(pool)
                .await
                .unwrap()
                .last_insert_rowid();
        let file_id = sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime) \
             VALUES (?, ?, 'book', 0, '')",
        )
        .bind(book_id)
        .bind(format)
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
        for (ordinal, filename, duration) in parts {
            sqlx::query(
                "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
                 VALUES (?, ?, ?, 0, 0, ?)",
            )
            .bind(file_id)
            .bind(*ordinal)
            .bind(*filename)
            .bind(*duration)
            .execute(pool)
            .await
            .unwrap();
        }
        uuid
    }

    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/manifest"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_404_for_unknown_uuid() {
        let (app, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/does-not-exist/manifest",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_direct_for_single_m4b() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_audiobook_with_parts(
            &pool,
            "/audiobooks",
            "M4B",
            &[(0, "Author/Book.m4b", 3600.0)],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/manifest"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "direct");
        assert_eq!(json["total_duration_seconds"].as_f64(), Some(3600.0));
        assert_eq!(json["parts"].as_array().unwrap().len(), 1);
        assert_eq!(json["parts"][0]["ordinal"], 0);
        assert_eq!(
            json["parts"][0]["url"],
            format!("/api/audiobooks/{uuid}/parts/0"),
        );
        assert_eq!(json["parts"][0]["mime"], "audio/mp4");
    }

    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_direct_for_mp3_folder() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_audiobook_with_parts(
            &pool,
            "/audiobooks",
            "MP3",
            &[
                (0, "Author/Book/01.mp3", 1800.0),
                (1, "Author/Book/02.mp3", 1800.0),
                (2, "Author/Book/03.mp3", 1800.0),
            ],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/manifest"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "direct");
        assert_eq!(json["total_duration_seconds"].as_f64(), Some(5400.0));
        let parts = json["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 3);
        assert_eq!(parts[2]["url"], format!("/api/audiobooks/{uuid}/parts/2"));
        assert_eq!(parts[0]["mime"], "audio/mpeg");
    }

    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_hls_when_any_part_is_flac() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        // One flac in an otherwise mp3 folder forces the whole book
        // through HLS — the cross-part timeline math doesn't have to
        // deal with mid-book codec switches that way.
        let uuid = seed_audiobook_with_parts(
            &pool,
            "/audiobooks",
            "MP3",
            &[
                (0, "Author/Book/01.mp3", 100.0),
                (1, "Author/Book/02.flac", 200.0),
            ],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/manifest"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["mode"], "hls");
        assert_eq!(
            json["playlist_url"],
            format!("/api/audiobooks/{uuid}/playlist.m3u8"),
        );
    }

    #[tokio::test]
    async fn api_get_audiobook_part_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/parts/0"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_audiobook_part_returns_404_when_book_is_hls_classified() {
        // Mixed-codec folders classify as HLS — the parts endpoint
        // must mirror that and 404 so clients can't bypass the
        // transcode pipeline by hitting `/parts` directly.
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_audiobook_with_parts(
            &pool,
            "/audiobooks",
            "MP3",
            &[
                (0, "Author/Book/01.mp3", 100.0),
                (1, "Author/Book/02.flac", 200.0),
            ],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/parts/0"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_audiobook_part_returns_404_for_out_of_range_ordinal() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_audiobook_with_parts(
            &pool,
            "/audiobooks",
            "MP3",
            &[(0, "Author/Book/01.mp3", 100.0)],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/parts/9"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_audiobook_part_serves_range_request_with_correct_mime() {
        // ServeFile reads the real file from disk, so we write a small
        // payload in a temp dir and point the seeded library at it. The
        // test asserts both Range support (206 + Content-Range) and the
        // mime override (`audio/mpeg`, not `application/octet-stream`).
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let library_path = dir.path().to_string_lossy().to_string();
        std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
        let file_path = dir.path().join("Author/Book/01.mp3");
        // 100-byte payload of alternating bytes so we can verify the
        // sliced Range bytes are exactly the slice we asked for.
        let payload: Vec<u8> = (0u8..100).collect();
        std::fs::File::create(&file_path)
            .unwrap()
            .write_all(&payload)
            .unwrap();

        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_audiobook_with_parts(
            &pool,
            &library_path,
            "MP3",
            &[(0, "Author/Book/01.mp3", 60.0)],
        )
        .await;

        let app = crate::backend::rest_router(AppState::new(pool));
        let req = axum::http::Request::builder()
            .uri(format!("/api/audiobooks/{uuid}/parts/0"))
            .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
            .header(axum::http::header::RANGE, "bytes=10-19")
            .body(axum::body::Body::empty())
            .unwrap();
        let res = app.oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
        let ct = res
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(ct, "audio/mpeg");
        let cr = res
            .headers()
            .get(axum::http::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(cr, "bytes 10-19/100");
        let body = axum::body::to_bytes(res.into_body(), usize::MAX)
            .await
            .unwrap();
        assert_eq!(body.as_ref(), &payload[10..=19]);
    }

    #[tokio::test]
    async fn api_get_audiobook_segment_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/audiobooks/some-uuid/segments/seg-0000.ts"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn is_valid_segment_name_rejects_traversal_paths() {
        assert!(!is_valid_segment_name("../secret.txt"));
        assert!(!is_valid_segment_name("seg-000.ts"));
        assert!(!is_valid_segment_name("seg-00000.ts"));
        assert!(!is_valid_segment_name("seg-abcd.ts"));
        assert!(is_valid_segment_name("seg-0000.ts"));
        assert!(is_valid_segment_name("seg-9999.ts"));
    }

    // -------------------------------------------------------------------
    // 5xx / DB-failure paths — induce sqlx errors by dropping the table
    // that the first DB call in each handler touches. Auth gate uses
    // `users`/`sessions` only, so it keeps passing; the handler's first
    // query hits "no such table" and falls into `internal(...)` → 500.
    // PRAGMA + DROP are pinned to a single pool connection because
    // `PRAGMA foreign_keys` is per-connection in SQLite — executing via
    // `&pool` would let the PRAGMA and the DROP land on different
    // connections, leaving FK enforcement ON and causing the DROP to
    // fail on FK constraints.
    // -------------------------------------------------------------------

    /// `get_audiobook_playlist` returns 500 when `resolve_audiobook` fails.
    /// Drop the `books` table after seeding auth — the gate keeps passing
    /// but the handler's JOIN hits "no such table: books".
    #[tokio::test]
    async fn api_get_audiobook_playlist_returns_500_when_db_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE books")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/any-uuid/playlist.m3u8",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_manifest` returns 500 when `resolve_audiobook` fails.
    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_500_when_db_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE books")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer("/api/audiobooks/any-uuid/manifest", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_manifest` returns 500 when `get_parts` fails. Seed a
    /// real audiobook so `resolve_audiobook` succeeds, then drop
    /// `book_file_parts` so the subsequent `get_parts` call errors out.
    #[tokio::test]
    async fn api_get_audiobook_manifest_returns_500_when_get_parts_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_one_audiobook(&pool).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE book_file_parts")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/manifest"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_part` returns 500 when `resolve_audiobook` fails.
    #[tokio::test]
    async fn api_get_audiobook_part_returns_500_when_db_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE books")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer("/api/audiobooks/any-uuid/parts/0", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_part` returns 500 when `get_parts` fails. Seed an mp3
    /// audiobook so `resolve_audiobook` succeeds, then drop
    /// `book_file_parts` so the subsequent `get_parts` call errors out.
    #[tokio::test]
    async fn api_get_audiobook_part_returns_500_when_get_parts_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let uuid = seed_one_audiobook(&pool).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE book_file_parts")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/audiobooks/{uuid}/parts/0"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_segment` returns 500 when `resolve_audiobook` fails.
    /// Passes a valid segment name (`seg-0000.ts`) so the name-validation
    /// guard passes before the DB call.
    #[tokio::test]
    async fn api_get_audiobook_segment_returns_500_when_db_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE books")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(
                "/api/audiobooks/any-uuid/segments/seg-0000.ts",
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }

    /// `get_audiobook_status` returns 500 when `resolve_audiobook` fails.
    #[tokio::test]
    async fn api_get_audiobook_status_returns_500_when_db_fails() {
        let (_, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let mut conn = pool.acquire().await.unwrap();
        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .unwrap();
        sqlx::query("DROP TABLE books")
            .execute(&mut *conn)
            .await
            .unwrap();
        drop(conn);

        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer("/api/audiobooks/any-uuid/status", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    }
}
