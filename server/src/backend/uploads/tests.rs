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

use omnibus_shared::{AudiobookInspection, Settings, UploadCommitResult, UploadInspection};

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

#[test]
fn extend_and_validate_magic_accepts_epub_magic_split_across_chunks() {
    let mut prefix = Vec::with_capacity(4);

    assert!(!extend_and_validate_magic(&mut prefix, b"P").unwrap());
    assert!(!extend_and_validate_magic(&mut prefix, b"K\x03").unwrap());
    assert!(extend_and_validate_magic(&mut prefix, b"\x04rest").unwrap());
    assert_eq!(prefix, b"PK\x03\x04");
}

#[test]
fn extend_and_validate_magic_rejects_invalid_split_prefix() {
    let mut prefix = Vec::with_capacity(4);

    assert!(!extend_and_validate_magic(&mut prefix, b"NO").unwrap());
    assert!(matches!(
        extend_and_validate_magic(&mut prefix, b"PE"),
        Err(UploadError::UnsupportedFormat)
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
            scan_interval_hours: None,
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
async fn commit_rejects_read_only_library_with_400() {
    let (app, _state, pool) = fixture().await;
    let library = tempfile::tempdir().expect("temp library dir");
    let library_path = library.path().to_string_lossy().to_string();
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some(library_path),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .expect("set library path");

    // Strip the write bit so filing the book hits PermissionDenied — the same
    // failure class as a `:ro` bind mount in the deployed container.
    let mut perms = std::fs::metadata(library.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
    std::fs::set_permissions(library.path(), perms).expect("chmod library read-only");

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        ("file", Some("book.epub"), &fixture_epub()),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = String::from_utf8_lossy(&bytes);
    assert!(
        msg.contains("not writable"),
        "error should name the read-only library, got: {msg}"
    );

    // Restore the write bit so the tempdir can be cleaned up.
    let mut perms = std::fs::metadata(library.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(library.path(), perms).expect("chmod library back");
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

// --- Audiobook ingest ------------------------------------------------------

/// Read a committed (non-download-gated) generated audiobook fixture.
fn fixture_audiobook(rel: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/audiobooks/generated")
        .join(rel);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// Minimal MP4 `ftyp` header — enough to pass the magic-byte gate (not lofty).
const M4B_MAGIC: &[u8] = b"\x00\x00\x00\x18ftypM4B \x00\x00\x00\x00isom";

async fn set_audiobook_library(pool: &sqlx::SqlitePool, path: &str) {
    db::set_settings(
        pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: Some(path.to_string()),
            scan_interval_hours: None,
        },
    )
    .await
    .expect("set audiobook library path");
}

#[tokio::test]
async fn audiobook_inspect_returns_metadata_for_single_mp3() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[(
        "file",
        Some("book.mp3"),
        &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
    )]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/audiobooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let inspection: AudiobookInspection = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(inspection.format, "mp3");
    assert_eq!(inspection.part_count, 1);
    assert!(inspection.title.is_some(), "fixture should yield a title");
}

#[tokio::test]
async fn audiobook_inspect_forbidden_without_upload_permission() {
    let (app, _state, pool) = fixture().await;
    let reader = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, reader.id).await;

    let (ct, body) = multipart_body(&[(
        "file",
        Some("book.mp3"),
        &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
    )]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/audiobooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn audiobook_commit_files_single_mp3_and_indexes() {
    let (app, _state, pool) = fixture().await;
    let _covers = CoversDirGuard::new("upload_audiobook_single");
    let library = tempfile::tempdir().expect("temp library dir");
    set_audiobook_library(&pool, &library.path().to_string_lossy()).await;

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Audio Title XYZ"),
        ("author", None, b"Audio Author"),
        (
            "file",
            Some("book.mp3"),
            &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
        ),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/audiobooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let commit: UploadCommitResult = serde_json::from_slice(&bytes).unwrap();
    assert!(!commit.uuid.is_empty());

    // A single mp3 lands in its own canonical folder (mp3 groups by folder).
    let placed = library
        .path()
        .join("audio-author")
        .join("audio-title-xyz")
        .join("book.mp3");
    assert!(placed.is_file(), "expected part at {}", placed.display());

    // The confirmed metadata reflects the user's edit, not the embedded tag.
    let book = db::get_book_by_uuid(&pool, &commit.uuid)
        .await
        .unwrap()
        .expect("uploaded audiobook should be indexed");
    assert_eq!(book.title.as_deref(), Some("Audio Title XYZ"));
    assert_eq!(
        book.creators.first().map(|c| c.name.as_str()),
        Some("Audio Author")
    );
}

#[tokio::test]
async fn audiobook_commit_files_multi_mp3_into_one_folder() {
    let (app, _state, pool) = fixture().await;
    let _covers = CoversDirGuard::new("upload_audiobook_multi");
    let library = tempfile::tempdir().expect("temp library dir");
    set_audiobook_library(&pool, &library.path().to_string_lossy()).await;

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Compiled Tales"),
        ("author", None, b"Grace Hopper"),
        (
            "file",
            Some("chapter01.mp3"),
            &fixture_audiobook("grace_hopper_series/the_compiled_tales/chapter01.mp3"),
        ),
        (
            "file",
            Some("chapter02.mp3"),
            &fixture_audiobook("grace_hopper_series/the_compiled_tales/chapter02.mp3"),
        ),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/audiobooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let commit: UploadCommitResult = serde_json::from_slice(&bytes).unwrap();

    // Both parts land in the same canonical folder → one book.
    let folder = library.path().join("grace-hopper").join("compiled-tales");
    assert!(folder.join("chapter01.mp3").is_file());
    assert!(folder.join("chapter02.mp3").is_file());

    let book = db::get_book_by_uuid(&pool, &commit.uuid)
        .await
        .unwrap()
        .expect("uploaded audiobook should be indexed");
    assert_eq!(book.title.as_deref(), Some("Compiled Tales"));
}

#[tokio::test]
async fn audiobook_commit_requires_configured_library_path() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        (
            "file",
            Some("book.mp3"),
            &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
        ),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/audiobooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audiobook_rejects_non_audio_with_415() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[("file", Some("evil.txt"), b"definitely not audio")]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/audiobooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn audiobook_rejects_multiple_single_containers_with_400() {
    let (app, _state, pool) = fixture().await;
    let library = tempfile::tempdir().expect("temp library dir");
    set_audiobook_library(&pool, &library.path().to_string_lossy()).await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // Two `.m4b` files can't be combined via upload — each is its own book.
    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        ("file", Some("part1.m4b"), M4B_MAGIC),
        ("file", Some("part2.m4b"), M4B_MAGIC),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/audiobooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audiobook_rejects_mp3_renamed_to_m4b_with_415() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // An mp3 payload with a `.m4b` extension: the family cross-check rejects it.
    let (ct, body) = multipart_body(&[(
        "file",
        Some("liar.m4b"),
        &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
    )]);
    let res = app
        .oneshot(post_multipart(
            "/api/uploads/audiobooks/inspect",
            &token,
            &ct,
            body,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}

#[tokio::test]
async fn audiobook_commit_rejects_read_only_library_with_400() {
    let (app, _state, pool) = fixture().await;
    let library = tempfile::tempdir().expect("temp library dir");
    set_audiobook_library(&pool, &library.path().to_string_lossy()).await;

    // Strip the write bit so filing hits PermissionDenied — same failure class
    // as a `:ro` bind mount.
    let mut perms = std::fs::metadata(library.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o555);
    std::fs::set_permissions(library.path(), perms).expect("chmod library read-only");

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Some Title"),
        ("author", None, b"Some Author"),
        (
            "file",
            Some("book.mp3"),
            &fixture_audiobook("ada_lovelace_solo/the_analytical_audiobook.mp3"),
        ),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/audiobooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = String::from_utf8_lossy(&bytes);
    assert!(
        msg.contains("not writable"),
        "error should name the read-only library, got: {msg}"
    );

    // Restore the write bit so the tempdir can be cleaned up.
    let mut perms = std::fs::metadata(library.path()).unwrap().permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(library.path(), perms).expect("chmod library back");
}
