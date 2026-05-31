// ---------------------------------------------------------------------------
// F2.1 Progress sync (REST — mobile client).
//
// One unified `/api/progress` endpoint accepts a discriminated payload
// (`{ format: "epub", epub_cfi }` or `{ format: "audio", audio_position_seconds }`)
// and fans out to per-format write paths inside `db::progress`. The same
// authenticated handler serves the (future) audiobook player. Web uses the
// `/api/rpc/progress*` server functions in `omnibus_frontend::rpc`.
// ---------------------------------------------------------------------------

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, progress::ProgressError};
use omnibus_shared::{ProgressFormat, ProgressUpdate, SessionReport};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::AuthUser;

#[derive(Debug, Deserialize)]
pub(super) struct ProgressQuery {
    #[serde(default = "default_format")]
    format: ProgressFormat,
}

fn default_format() -> ProgressFormat {
    ProgressFormat::Epub
}

/// Persist a new reading/listening position. Last-write-wins on
/// `(user, book, format)`; returns the server-authoritative record so the
/// caller can sync forward.
pub(super) async fn post_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<ProgressUpdate>,
) -> Response {
    if let Err(msg) = update.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::progress::upsert_progress(&state.pool, user.id, &update).await {
        Ok(rec) => Json(rec).into_response(),
        Err(ProgressError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ProgressError::Sqlx(e)) => internal("upsert_progress", e),
    }
}

/// Fetch the current position for `(user, uuid, format)`. `format` defaults
/// to `epub` when omitted. Returns `200 { … }` with an `Option<ProgressRecord>`
/// body (`null` when the user has not yet opened the book in that format).
pub(super) async fn get_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(q): Query<ProgressQuery>,
) -> Response {
    match db::progress::get_progress(&state.pool, user.id, &uuid, q.format).await {
        Ok(rec) => Json(rec).into_response(),
        Err(e) => internal("get_progress", e),
    }
}

/// Append a batch of session reports. Mobile posts these on reconnect; web
/// posts best-effort on unmount. Each report is validated at the API
/// boundary (negative durations / inverted time ranges → 400); unknown
/// book uuids are silently skipped inside the db layer (best-effort
/// telemetry). `recorded` reflects the **inserted** row count so callers
/// can tell which queued reports actually persisted.
pub(super) async fn post_sessions(
    user: AuthUser,
    State(state): State<AppState>,
    Json(reports): Json<Vec<SessionReport>>,
) -> Response {
    for r in &reports {
        if let Err(msg) = r.validate() {
            return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
        }
    }
    let mut inserted = 0usize;
    for r in &reports {
        match db::progress::record_session(&state.pool, user.id, r).await {
            Ok(true) => inserted += 1,
            Ok(false) => {}
            Err(e) => return internal("record_session", e),
        }
    }
    Json(serde_json::json!({ "recorded": inserted })).into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use omnibus_shared::ProgressRecord;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    #[tokio::test]
    async fn api_post_progress_requires_auth() {
        let (app, _state, _pool) = fixture().await;
        let body = serde_json::json!({
            "book_uuid": "x",
            "format": "epub",
            "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_post_progress_rejects_missing_cfi() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!({
            "book_uuid": "anything",
            "format": "epub",
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_post_progress_rejects_cross_format_field() {
        // `{format:"epub", audio_position_seconds:…}` must 400 at the
        // boundary so the migration's CHECK doesn't surface as a 500
        // (issue: copilot review on #300).
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!({
            "book_uuid": "anything",
            "format": "epub",
            "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
            "audio_position_seconds": 12.0,
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_post_progress_404_on_unknown_book() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!({
            "book_uuid": "no-such-uuid",
            "format": "epub",
            "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_progress_round_trip_last_write_wins() {
        let (app, _state, pool) = fixture().await;
        let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        // First write.
        let body = serde_json::json!({
            "book_uuid": uuid,
            "format": "epub",
            "epub_cfi": "epubcfi(/6/4!/4/2/1:0)",
        });
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Second write — overwrites.
        let body = serde_json::json!({
            "book_uuid": uuid,
            "format": "epub",
            "epub_cfi": "epubcfi(/6/12!/4/8/3:7)",
        });
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/progress")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);

        // Read back — last write wins.
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/progress/{uuid}?format=epub"))
                    .method("GET")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let rec: Option<ProgressRecord> = serde_json::from_slice(&bytes).unwrap();
        let rec = rec.unwrap();
        assert_eq!(rec.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
        assert_eq!(rec.format, ProgressFormat::Epub);
    }

    #[tokio::test]
    async fn api_post_sessions_records_count() {
        let (app, _state, pool) = fixture().await;
        let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!([
            {
                "book_uuid": uuid,
                "format": "epub",
                "started_at": 100,
                "ended_at": 460,
                "progress_units": 360,
            }
        ]);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress/sessions")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["recorded"], 1);
    }

    #[tokio::test]
    async fn api_post_sessions_reports_only_inserted_when_some_skipped() {
        // One real uuid + one unknown uuid. Handler must report
        // `recorded: 1`, not 2 — silently-skipped reports are tracked
        // separately so the mobile client can detect data loss (issue:
        // copilot review on #300).
        let (app, _state, pool) = fixture().await;
        let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!([
            { "book_uuid": uuid,             "format": "epub", "started_at": 100, "ended_at": 460, "progress_units": 360 },
            { "book_uuid": "no-such-uuid",   "format": "epub", "started_at": 100, "ended_at": 460, "progress_units": 360 },
        ]);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress/sessions")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["recorded"], 1);
    }

    #[tokio::test]
    async fn api_post_sessions_rejects_inverted_time_range() {
        let (app, _state, pool) = fixture().await;
        let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!([
            { "book_uuid": uuid, "format": "epub", "started_at": 500, "ended_at": 200, "progress_units": 0 }
        ]);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/progress/sessions")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }
}
