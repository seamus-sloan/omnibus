//! Tests for the cross-format resume REST endpoint: auth, unknown-book
//! 404, the candidate-less states, a mapped candidate, and DB-failure 5xx.

use axum::body::to_bytes;
use axum::http::StatusCode;
use omnibus_shared::cross_format::{
    CrossFormatLinkMode, CrossFormatResume, CrossFormatResumeState,
};
use omnibus_shared::{ProgressFormat, ProgressUpdate};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use omnibus_db as db;

async fn body_json(res: axum::response::Response) -> CrossFormatResume {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// Give the seeded book one M4B file with a single 600s part; returns the
/// `book_files.id`.
async fn attach_audio(pool: &sqlx::SqlitePool, book_id: i64) -> i64 {
    let file_id: i64 = sqlx::query_scalar(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
         VALUES (?, 'M4B', 'audio', 100, 1000, 'audio.m4b', 0) RETURNING id",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_file_parts
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
         VALUES (?, 0, 'audio.m4b', 100, 1000, 600.0)",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
    file_id
}

#[tokio::test]
async fn api_cross_format_resume_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/books/x/cross-format-resume?target=audio"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_cross_format_resume_404s_for_an_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/books/no-such/cross-format-resume?target=audio",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_cross_format_resume_reports_not_linked_by_default() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/cross-format-resume?target=audio"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resume = body_json(res).await;
    assert_eq!(resume.state, CrossFormatResumeState::NotLinked);
    assert!(resume.candidate.is_none());
}

#[tokio::test]
async fn api_cross_format_resume_serves_a_mapped_candidate() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    let file_id = attach_audio(&pool, book_id).await;
    db::cross_format::upsert_link(&pool, user.id, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    db::progress::upsert_progress(
        &pool,
        user.id,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(50),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(2_000),
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/cross-format-resume?target=audio"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resume = body_json(res).await;
    assert_eq!(resume.state, CrossFormatResumeState::Candidate);
    let c = resume.candidate.unwrap();
    assert_eq!(c.book_file_id, Some(file_id));
    assert_eq!(c.audio_position_seconds, Some(300.0));
    assert_eq!(c.total_duration_seconds, Some(600.0));
    assert_eq!(c.source_client_updated_at, 2_000);
}

#[tokio::test]
async fn api_cross_format_resume_500s_when_the_db_is_gone() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    sqlx::query("DROP TABLE cross_format_links")
        .execute(&pool)
        .await
        .unwrap();
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/cross-format-resume?target=audio"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Authed user + dual-format book with `cross_format_links` dropped, so every
/// cross-format query fails at the DB layer.
async fn fixture_with_no_link_table() -> (axum::Router, String, String) {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    attach_audio(&pool, book_id).await;
    sqlx::query("DROP TABLE cross_format_links")
        .execute(&pool)
        .await
        .unwrap();
    (app, token, uuid)
}

#[tokio::test]
async fn api_get_alignment_500s_when_the_db_is_gone() {
    let (app, token, uuid) = fixture_with_no_link_table().await;
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/alignment"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_post_cross_format_link_500s_when_the_db_is_gone() {
    let (app, token, uuid) = fixture_with_no_link_table().await;
    // A body that clears `validate()` — the DB is only reached past it.
    let res = app
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/cross-format-link"),
            &token,
            &serde_json::json!({ "book_uuid": "ignored", "mode": "sequence" }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_delete_cross_format_link_500s_when_the_db_is_gone() {
    let (app, token, uuid) = fixture_with_no_link_table().await;
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/books/{uuid}/cross-format-link"))
                .method("DELETE")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                .body(axum::body::Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_post_sync_point_500s_when_the_db_is_gone() {
    let (app, token, uuid) = fixture_with_no_link_table().await;
    // Audio-with-seconds clears `validate()`, so this reaches the link read.
    let res = app
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/sync-point"),
            &token,
            &serde_json::json!({
                "book_uuid": "ignored",
                "format": "audio",
                "audio_seconds": 300.0,
            }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_post_cross_format_follow_500s_when_the_db_is_gone() {
    let (app, token, uuid) = fixture_with_no_link_table().await;
    let res = app
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/cross-format-follow"),
            &token,
            &serde_json::json!({ "enabled": true }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_alignment_and_link_lifecycle_round_trips() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    attach_audio(&pool, book_id).await;

    // Alignment read renders (no link yet).
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/alignment"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Confirm via REST → 204; resume state leaves not_linked.
    let body = serde_json::json!({ "book_uuid": "ignored", "mode": "sequence" });
    let res = app
        .clone()
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/books/{uuid}/cross-format-link"))
                .method("POST")
                .header("content-type", "application/json")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/cross-format-resume?target=audio"),
            &token,
        ))
        .await
        .unwrap();
    let resume = body_json(res).await;
    assert_ne!(resume.state, CrossFormatResumeState::NotLinked);

    // Unlink via REST → 204, idempotent.
    for _ in 0..2 {
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!("/api/books/{uuid}/cross-format-link"))
                    .method("DELETE")
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }
}

#[tokio::test]
async fn api_cross_format_link_gates_reorder_on_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "norights").await;
    sqlx::query("UPDATE users SET can_edit = 0 WHERE id = ?")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    let file_id = attach_audio(&pool, book_id).await;

    let body = serde_json::json!({
        "book_uuid": "ignored",
        "mode": "sequence",
        "audio_order": [file_id],
    });
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .uri(format!("/api/books/{uuid}/cross-format-link"))
                .method("POST")
                .header("content-type", "application/json")
                .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                .body(axum::body::Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

/// Bearer-authed JSON POST, shared by the declaration/follow tests.
fn post_json_with_bearer(
    uri: &str,
    token: &str,
    body: &serde_json::Value,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", "application/json")
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn api_sync_point_declares_and_conflicts_map_to_409() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    attach_audio(&pool, book_id).await;
    db::progress::upsert_progress(
        &pool,
        user.id,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(40),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1_000),
        },
    )
    .await
    .unwrap();

    // Happy path: single-file book auto-links, records the anchor, and
    // the follow flag reaches the resume payload.
    let body = serde_json::json!({
        "book_uuid": "ignored",
        "format": "audio",
        "audio_seconds": 300.0,
    });
    let res = app
        .clone()
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/sync-point"),
            &token,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/books/{uuid}/cross-format-resume?target=audio"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let resume = body_json(res).await;
    assert!(resume.follow);

    // A declaration with no counterpart position → 409 with a renderable
    // message (fresh user, no reading row).
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;
    let res = app
        .clone()
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/sync-point"),
            &bob_token,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Malformed declaration (audio without seconds) → 422.
    let res = app
        .clone()
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/sync-point"),
            &token,
            &serde_json::json!({"book_uuid": "x", "format": "audio"}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_cross_format_follow_flips_and_conflicts_without_a_link() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (book_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dual").await;
    attach_audio(&pool, book_id).await;

    let res = app
        .clone()
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/cross-format-follow"),
            &token,
            &serde_json::json!({"enabled": true}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    db::cross_format::upsert_link(&pool, user.id, &uuid, CrossFormatLinkMode::Sequence, None)
        .await
        .unwrap();
    let res = app
        .oneshot(post_json_with_bearer(
            &format!("/api/books/{uuid}/cross-format-follow"),
            &token,
            &serde_json::json!({"enabled": true}),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(
        db::cross_format::get_link(&pool, user.id, &uuid)
            .await
            .unwrap()
            .unwrap()
            .follow
    );
}
