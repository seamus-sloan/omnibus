//! Integration tests for the single-book OPF export endpoint.
//!
//! The RPC variant (`rpc_export_opf`) shares the same db call and is covered
//! transitively by `db::opf_export` unit tests; these cover the REST entry
//! point the mobile client uses — auth, permission, resolution, and the
//! written-sidecar success path.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::OpfExportResult;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn post_export(uuid: &str, token: Option<&str>) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(format!("/api/ebooks/{uuid}/export-opf"))
        .method("POST");
    if let Some(token) = token {
        builder = builder.header(AUTHORIZATION, format!("Bearer {token}"));
    }
    builder.body(Body::empty()).unwrap()
}

#[tokio::test]
async fn api_export_opf_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(post_export("uuid-1", None))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_export_opf_returns_403_for_user_without_edit_permission() {
    let (app, _state, pool) = fixture().await;
    let lib = tempfile::tempdir().unwrap();
    let (_id, uuid) = db::test_support::seed_epub_book_at(&pool, lib.path()).await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post_export(&uuid, Some(&token)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(
        !lib.path().join("sub").join("metadata.opf").exists(),
        "403 path must not write a sidecar"
    );
}

#[tokio::test]
async fn api_export_opf_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(post_export("does-not-exist", Some(&token)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_export_opf_writes_sidecar_and_returns_path_and_backed_up() {
    let (app, _state, pool) = fixture().await;
    let lib = tempfile::tempdir().unwrap();
    let (_id, uuid) = db::test_support::seed_epub_book_at(&pool, lib.path()).await;
    // A can_edit non-admin user must be able to export (AC3).
    let user = auth_test_support::create_user(&pool, "editor").await;
    sqlx::query("UPDATE users SET can_edit = 1 WHERE id = ?")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(post_export(&uuid, Some(&token)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let result: OpfExportResult = serde_json::from_slice(&bytes).unwrap();
    assert!(result.path.ends_with("sub/metadata.opf"), "{}", result.path);
    assert!(!result.backed_up);

    let written = std::fs::read_to_string(lib.path().join("sub").join("metadata.opf")).unwrap();
    assert!(written.contains("<dc:title>Title</dc:title>"));
}

#[tokio::test]
async fn api_export_opf_returns_409_when_book_has_no_epub() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // An audiobook-only book: a books row with only an M4B file.
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/audio', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified)
         VALUES ('uuid-ab', 'sub/audio', 1, 'sub', 'Audio', '2000-01-01 00:00:00')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-ab'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'M4B', 'audio', 4, 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let res = app
        .oneshot(post_export("uuid-ab", Some(&token)))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::CONFLICT);
}
