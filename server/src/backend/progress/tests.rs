//! Tests for progress-sync REST handlers.
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

#[tokio::test]
async fn api_post_sessions_batch_of_two_records_both() {
    // Two valid reports in one POST must both land atomically; `recorded`
    // must reflect the full count (not a partial commit).
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        { "book_uuid": uuid, "format": "epub",  "started_at": 100, "ended_at": 460, "progress_units": 360 },
        { "book_uuid": uuid, "format": "audio", "started_at": 500, "ended_at": 900, "progress_units": 400 },
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
    assert_eq!(v["recorded"], 2);
}

#[tokio::test]
async fn post_sessions_rejects_oversized_batch() {
    // Batches larger than SESSION_BATCH_CAP must be rejected with 422
    // before any DB work — the global 1 MiB body limit permits thousands
    // of compact reports, so the per-batch cap is the real defense
    // against a client holding the WAL write lock open.
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let reports: Vec<_> = (0..=SESSION_BATCH_CAP)
        .map(|_| {
            serde_json::json!({
                "book_uuid": uuid,
                "format": "epub",
                "started_at": 100,
                "ended_at": 460,
                "progress_units": 360,
            })
        })
        .collect();
    let body = serde_json::Value::Array(reports);
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
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
    let written: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user.id)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        written, 0,
        "oversized batch must reject before any DB write"
    );
}

#[tokio::test]
async fn api_post_sessions_batch_resolves_direct_and_merged_uuids_in_one_prefetch() {
    // Issue #633: the batch handler pre-resolves every uuid in one bulk query,
    // then loops through INSERT-only work. This test seeds a mixed batch —
    // a direct `books.uuid`, a `merged_uuids` alias, and an unknown uuid — and
    // asserts each report lands against the *canonical* survivor uuid (the
    // merged-uuid alias must collapse onto the surviving book) and that the
    // unknown row is silently dropped from `recorded`.
    let (app, _state, pool) = fixture().await;
    let (book_id, survivor_uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('alias-uuid', ?, 'epub', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let body = serde_json::json!([
        { "book_uuid": survivor_uuid, "format": "epub", "started_at": 100, "ended_at": 460, "progress_units": 360 },
        { "book_uuid": "alias-uuid", "format": "epub", "started_at": 500, "ended_at": 800, "progress_units": 300 },
        { "book_uuid": "no-such-uuid", "format": "epub", "started_at": 900, "ended_at": 950, "progress_units": 50 },
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
    assert_eq!(v["recorded"], 2, "direct + merged land; unknown skipped");
    // Both rows must key on the *canonical* survivor uuid — the merged alias
    // resolves through the bulk map, not against its literal string.
    let against_survivor: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user.id)
    .bind(&survivor_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        against_survivor, 2,
        "both reports land against canonical uuid"
    );
    let against_alias: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = 'alias-uuid'",
    )
    .bind(user.id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        against_alias, 0,
        "no row must key on the merged alias string"
    );
}

#[tokio::test]
async fn api_post_sessions_rollback_on_second_insert_error() {
    // Drop the audio-session table so the 2nd insert fails deterministically.
    // This proves the batch-level transaction rolls back: the 1st (epub)
    // row must NOT be committed even though the insert was called.
    let (app, _state, pool) = fixture().await;
    let (_book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    sqlx::query("DROP TABLE listening_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let body = serde_json::json!([
        { "book_uuid": uuid, "format": "epub",  "started_at": 100, "ended_at": 460, "progress_units": 360 },
        { "book_uuid": uuid, "format": "audio", "started_at": 500, "ended_at": 900, "progress_units": 400 },
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

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user.id)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 0,
        "rolled-back transaction must leave no reading_sessions rows"
    );
}

#[tokio::test]
async fn api_get_recent_progress_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/recent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_recent_progress_returns_resume_points_newest_first() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (_, uuid_a) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let (_, uuid_b) = seed_book_with_uuid(&pool, "/lib", "Book B").await;
    for uuid in [&uuid_a, &uuid_b] {
        omnibus_db::progress::upsert_progress(
            &pool,
            user.id,
            &omnibus_shared::ProgressUpdate {
                book_uuid: uuid.clone(),
                format: omnibus_shared::ProgressFormat::Epub,
                epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
                audio_position_seconds: None,
                client_updated_at: None,
            },
        )
        .await
        .unwrap();
    }
    // Same-second upserts tie on client_updated_at (the recency column
    // `recent_progress` now orders on); force Book B newest on both columns.
    sqlx::query(
        "UPDATE reading_progress SET updated_at = 200, client_updated_at = 200 WHERE book_uuid = ?",
    )
    .bind(&uuid_b)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE reading_progress SET updated_at = 100, client_updated_at = 100 WHERE book_uuid = ?",
    )
    .bind(&uuid_a)
    .execute(&pool)
    .await
    .unwrap();

    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/progress/recent?limit=1")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let points: Vec<omnibus_shared::ResumePoint> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].record.book_uuid, uuid_b);
    assert_eq!(points[0].book.title.as_deref(), Some("Book B"));
}
