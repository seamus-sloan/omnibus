//! F2.3 HLS audiobook streaming — playlist manifest, segment file, and
//! transcode-readiness status endpoints.
//!
//! Three handlers replace the old `/api/audiobooks/{uuid}/file` route:
//!
//! - `GET /api/audiobooks/{uuid}/playlist.m3u8` — returns a VOD HLS manifest
//!   built from the stored `book_file_parts.duration_seconds` values. Always
//!   present once a book is indexed, even before the transcode finishes.
//! - `GET /api/audiobooks/{uuid}/segments/{segment}` — serves `seg-NNNN.ts`
//!   files from the on-disk HLS segment cache. Triggers a transcode job if the
//!   segment is missing (fire-and-wait so the first segment is usually ready by
//!   the time hls.js asks for it).
//! - `GET /api/audiobooks/{uuid}/status` — returns `{"ready":bool,"progress":f32}`
//!   so the frontend can show a "Preparing…" overlay while the first transcode
//!   runs and poll until ready before initializing hls.js.

use axum::{
    body::Body,
    extract::{Path, Request, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{hls, worker::Task};
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

    let parts = match hls::get_parts(&state.pool, resolved.book_file_id).await {
        Ok(p) => p,
        Err(e) => return internal("get_parts", e),
    };

    let m3u8 = hls::build_manifest(&parts);
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
    let progress = hls::read_progress(book_id, hls::AUDIO64);

    // If neither ready nor in progress, kick off the transcode. The worker
    // serializes duplicate posts via the per-resource keyed mutex so a second
    // poll while the job is already running just enqueues behind it.
    if !ready && progress < 0.05 {
        state.worker.post(Task::HlsTranscode {
            book_id,
            book_file_id: resolved.book_file_id,
            library_path: resolved.library_path,
            profile: hls::AUDIO64.to_string(),
        });
    }

    Json(StatusResponse { ready, progress }).into_response()
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

/// JSON response body for `GET /api/audiobooks/{uuid}/status`.
#[derive(Serialize)]
struct StatusResponse {
    ready: bool,
    progress: f32,
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
    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

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
    async fn api_get_audiobook_status_returns_ready_false_when_not_transcoded() {
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
}
