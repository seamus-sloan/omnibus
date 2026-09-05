//! The EPUB path: the byte validators (magic, size cap, magic split
//! across chunks), inspect for admins and `can_upload` users with its 415
//! and 403, every declared creator surviving an author edit, and commit's
//! library-path, oversized-field, read-only-library and non-EPUB
//! rejections.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use omnibus_shared::{Settings, UploadCommitResult, UploadInspection};

use super::super::*;
use super::{multipart_body, post_multipart};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Read a small committed EPUB fixture (shared with the Playwright suite).
fn fixture_epub() -> Vec<u8> {
    fixture_epub_named("standalone-desert.epub")
}

/// Read one of the committed generated EPUBs by file name.
fn fixture_epub_named(name: &str) -> Vec<u8> {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// `beta.epub` declares two `dc:creator`s — the multi-creator case (#2355).
const TWO_CREATOR_EPUB: &str = "beta.epub";

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
async fn inspect_returns_every_creator_the_file_declares() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[(
        "file",
        Some("book.epub"),
        &fixture_epub_named(TWO_CREATOR_EPUB),
    )]);
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
    assert_eq!(inspection.author.as_deref(), Some("Grace Hopper"));
    assert_eq!(
        inspection.creators,
        vec!["Grace Hopper".to_string(), "Margaret Hamilton".to_string()]
    );
}

/// The renamed lead creator keeps its refinements and loses only its author
/// row; the co-author rides along untouched (#2355).
#[test]
fn edited_creators_keeps_the_first_creators_refinements() {
    let embedded = vec![
        Contributor {
            name: "Hopper, G.".to_string(),
            role: Some("aut".to_string()),
            file_as: Some("Hopper, Grace".to_string()),
            id: Some(7),
        },
        Contributor {
            name: "Margaret Hamilton".to_string(),
            role: None,
            file_as: None,
            id: Some(8),
        },
    ];
    let out = edited_creators("Grace Hopper".to_string(), &embedded);
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].name, "Grace Hopper");
    assert_eq!(out[0].role.as_deref(), Some("aut"));
    assert_eq!(out[0].file_as.as_deref(), Some("Hopper, Grace"));
    assert_eq!(out[0].id, None);
    assert_eq!(out[1], embedded[1]);
}

/// Editing the Author field replaces the first creator only; the co-author
/// the file declared survives the commit (#2355).
#[tokio::test]
async fn commit_keeps_additional_creators_when_the_author_is_edited() {
    let (app, _state, pool) = fixture().await;
    let _covers = CoversDirGuard::new("upload_commit_creators");
    let library = tempfile::tempdir().expect("temp library dir");
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some(library.path().to_string_lossy().to_string()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .expect("set library path");

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Beta in the Series"),
        ("author", None, b"Grace B. Hopper"),
        (
            "file",
            Some("book.epub"),
            &fixture_epub_named(TWO_CREATOR_EPUB),
        ),
    ]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::CREATED);

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let commit: UploadCommitResult = serde_json::from_slice(&bytes).unwrap();
    let book = db::get_book_by_uuid(&pool, &commit.uuid)
        .await
        .unwrap()
        .expect("uploaded book should be indexed");
    let names: Vec<&str> = book.creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["Grace B. Hopper", "Margaret Hamilton"]);
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
async fn commit_rejects_oversized_title_field_with_413_before_full_buffering() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // The oversized text field is rejected while streaming, before the parser
    // even reaches the (absent here) `file` field.
    let oversized_title = "a".repeat(MAX_TEXT_FIELD_BYTES + 1);
    let (ct, body) = multipart_body(&[("title", None, oversized_title.as_bytes())]);
    let res = app
        .oneshot(post_multipart("/api/uploads/ebooks", &token, &ct, body))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
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
