//! `/api/covers/*` and `/api/thumbs/*` handlers.
//!
//! Session-gated reads that resolve a book uuid through the db layer and
//! stream the cached cover or WebP thumbnail bytes back to the client.
//! Authenticated via [`MediaAuthUser`] (cookie / bearer / `?token=` query),
//! the last so the mobile WebView's `<img>` fetches can carry the session.
//! Mounted on the mobile REST router in [`super::rest_router`].

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::MediaAuthUser;

pub(super) async fn get_cover(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    // Resolve uuid → id so the existing id-keyed `db::get_cover` (which
    // reads cover bytes from `<covers_dir>/<uuid>.<ext>` by way of the
    // books row) stays unchanged. The route surface is uuid-keyed so
    // bookmarked URLs survive reindexes; the storage layer keeps using
    // the autoincrement id internally for join performance.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Covers are static per-book (new id on reindex). Cached on
                // the client only — `private` + `Vary: Cookie` keep a shared
                // proxy from serving one user's covers to an unauthenticated
                // request on the same URL now that the endpoint is gated.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                // Prevent browsers from MIME-sniffing a cover into an
                // executable type (e.g. an SVG disguised as JPEG).
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read cover", error),
    }
}

pub(super) async fn get_thumb(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path((uuid, size_str)): Path<(String, String)>,
) -> Response {
    let size: db::ThumbSize = match size_str.parse() {
        Ok(s) => s,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid size; use sm, md, or lg",
            )
                .into_response();
        }
    };
    // uuid → id translation: see the comment in `get_cover` for why we
    // resolve at the edge rather than rewriting the thumbnail pipeline
    // to be uuid-keyed end-to-end.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    let last_modified_epoch = match db::get_last_modified_epoch(&state.pool, id).await {
        Ok(Some(ts)) => ts,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("read last_modified_epoch", e),
    };

    // Cache hit: thumb exists and is fresh. Use async I/O here so a hot
    // `srcset` grid doesn't pin tokio worker threads on the synchronous read.
    let thumb_path = db::thumb_path_for(id, size);
    if !db::thumbs::is_stale_async(id, size, last_modified_epoch).await {
        if let Ok(bytes) = tokio::fs::read(&thumb_path).await {
            return (
                [
                    (header::CONTENT_TYPE, "image/webp"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                    (header::VARY, "Cookie"),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // Cache miss or stale: fetch the original cover first so we only queue
    // generation when there's actually something to thumbnail. Queuing for
    // a coverless book just produces a guaranteed `no cover for book …`
    // worker error on every request, polluting the log.
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => {
            state.worker.post(db::worker::Task::GenerateThumbs {
                book_id: id,
                last_modified_epoch,
            });
            (
                [
                    (header::CONTENT_TYPE, mime.as_str()),
                    // Short TTL: browser will re-fetch after ~5 s when the WebP is ready.
                    (header::CACHE_CONTROL, "private, max-age=5"),
                    (header::VARY, "Cookie"),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => axum::http::StatusCode::ACCEPTED.into_response(),
        Err(e) => internal("cover fetch for thumb", e),
    }
}

#[cfg(test)]
mod tests {
    use axum::http::StatusCode;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    #[tokio::test]
    async fn api_get_covers_returns_not_found_for_missing_id() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/covers/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_covers_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/covers/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_thumbs_returns_400_for_bad_size() {
        let (app, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/1/xxl", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_thumbs_returns_404_for_missing_book() {
        let (app, _, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/9999/md", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_thumbs_returns_202_for_book_without_cover() {
        let (_, _, pool) = fixture().await;
        // seed_book_no_cover uses this fixed uuid; route is uuid-keyed now.
        let _ = seed_book_no_cover(&pool).await;
        let uuid = "00000000-0000-0000-0000-000000000001";
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let app = crate::backend::rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn api_thumbs_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/thumbs/1/md")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
