//! Integration tests for the "add your own books" upload endpoints.
//!
//! Inspect/commit are driven through `rest_router(...).oneshot(...)` against an
//! in-memory DB. The commit happy-path drives a real worker scan (the same
//! pattern the settings reindex test uses) so the indexer actually inserts the
//! book before the override is layered on top.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use tower::ServiceExt;

use omnibus_shared::{Settings, UploadCommitResult, UploadInspection};

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Read a small committed EPUB fixture (shared with the Playwright suite).
fn fixture_epub() -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated/standalone-desert.epub");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// Build a `multipart/form-data` body. Each part is
/// `(field_name, optional_filename, content)`; a filename marks a file part.
fn multipart_body(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----omnibus-upload-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, filename, content) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(fname) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn post_multipart(uri: &str, token: &str, content_type: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", content_type)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap()
}

/// Insert a non-admin user that carries `can_upload` but not `is_admin`, so we
/// can prove the permission gate accepts the dedicated upload flag.
async fn create_uploader(pool: &sqlx::SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!test-no-password', 0, 1, 0, 1) RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .expect("insert uploader")
}

// --- Pure validation -------------------------------------------------------

#[test]
fn validate_file_bytes_accepts_zip_magic_under_cap() {
    assert!(validate_file_bytes(b"PK\x03\x04ok", 100).is_ok());
}

#[test]
fn validate_file_bytes_rejects_non_epub() {
    assert!(matches!(
        validate_file_bytes(b"not an epub at all", 100),
        Err(UploadError::UnsupportedFormat)
    ));
}

#[test]
fn validate_file_bytes_rejects_oversize() {
    let mut big = vec![0u8; 200];
    big[..4].copy_from_slice(b"PK\x03\x04");
    assert!(matches!(
        validate_file_bytes(&big, 100),
        Err(UploadError::TooLarge(100))
    ));
}

// --- Inspect ---------------------------------------------------------------

#[tokio::test]
async fn inspect_returns_extracted_metadata_for_admin() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[("file", Some("book.epub"), &fixture_epub())]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/ebooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let inspection: UploadInspection = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inspection.ext, "epub");
    assert!(
        inspection.title.is_some(),
        "fixture should yield an embedded title"
    );
}

#[tokio::test]
async fn inspect_allows_non_admin_with_can_upload() {
    let (app, _state, pool) = fixture().await;
    let uploader = create_uploader(&pool, "uploader").await;
    let token = auth_test_support::bearer_token(&pool, uploader).await;

    let (ct, body) = multipart_body(&[("file", Some("book.epub"), &fixture_epub())]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/ebooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(
        res.status(),
        StatusCode::OK,
        "a can_upload user must pass the gate"
    );
}

#[tokio::test]
async fn inspect_rejects_non_epub_with_415() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[("file", Some("evil.txt"), b"definitely not an epub")]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/ebooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn inspect_forbidden_without_upload_permission() {
    let (app, _state, pool) = fixture().await;
    // `create_user` has can_upload = false and is_admin = false.
    let reader = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, reader.id).await;

    let (ct, body) = multipart_body(&[("file", Some("book.epub"), &fixture_epub())]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/ebooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

// --- Commit ----------------------------------------------------------------

#[tokio::test]
async fn commit_files_book_and_applies_edited_metadata() {
    let (app, _state, pool) = fixture().await;
    // Isolate the cover cache so the reindex doesn't write into the repo.
    let _covers = CoversDirGuard::new("upload_commit");
    let library = tempfile::tempdir().expect("temp library dir");
    let library_path = library.path().to_string_lossy().to_string();
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some(library_path.clone()),
            audiobook_library_path: None,
        },
    )
    .await
    .expect("set library path");

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Edited Title XYZ"),
        ("author", None, b"Edited Author"),
        ("file", Some("book.epub"), &fixture_epub()),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let commit: UploadCommitResult = serde_json::from_slice(&bytes).unwrap();
    assert!(!commit.uuid.is_empty());

    // The file landed in the canonical author-slug/title-slug folder.
    let expected = library
        .path()
        .join("edited-author")
        .join("edited-title-xyz")
        .join("edited-title-xyz.epub");
    assert!(
        expected.is_file(),
        "expected uploaded file at {}",
        expected.display()
    );

    // The displayed metadata reflects the user's edit, not the embedded title.
    let book = db::get_book_by_uuid(&pool, &commit.uuid)
        .await
        .unwrap()
        .expect("uploaded book should be indexed");
    assert_eq!(book.title.as_deref(), Some("Edited Title XYZ"));
    assert_eq!(
        book.creators.first().map(|c| c.name.as_str()),
        Some("Edited Author")
    );
}

#[tokio::test]
async fn commit_requires_configured_library_path() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Valid magic bytes so we get past the format gate and reach the settings
    // check, but no library path is configured on the fresh DB.
    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        ("file", Some("book.epub"), b"PK\x03\x04fake-epub-bytes"),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn commit_rejects_non_epub_with_415() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        ("file", Some("evil.txt"), b"not an epub"),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

// --- Audiobook stub --------------------------------------------------------

#[tokio::test]
async fn audiobook_endpoints_are_stubbed_with_501() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    for uri in ["/api/uploads/audiobooks", "/api/uploads/audiobooks/inspect"] {
        let (ct, body) = multipart_body(&[("file", Some("a.m4b"), b"\x00\x00\x00\x18ftyp")]);
        let res = app
            .clone()
            .oneshot(post_multipart(uri, &token, &ct, body))
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::NOT_IMPLEMENTED,
            "{uri} should be stubbed"
        );
    }
}
