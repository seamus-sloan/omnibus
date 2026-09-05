//! The audiobook path: inspect and commit for a single MP3 and a
//! multi-MP3 folder, the supplied series persisted as an override, the
//! permission and library-path gates, and the 415 / 400 rejections for
//! non-audio, multiple single containers, a renamed MP3, unparseable tags
//! and a read-only library.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use omnibus_shared::{AudiobookInspection, Settings, UploadCommitResult};

use super::super::*;
use super::{multipart_body, post_multipart};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

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

/// The audiobook confirm form offers Series / Series index (#2254), and an
/// audiobook container almost never carries a series statement of its own —
/// so what the reader types there has to land on the book record.
#[tokio::test]
async fn audiobook_commit_persists_the_supplied_series_as_an_override() {
    let (app, _state, pool) = fixture().await;
    let _covers = CoversDirGuard::new("upload_audiobook_series");
    let library = tempfile::tempdir().expect("temp library dir");
    set_audiobook_library(&pool, &library.path().to_string_lossy()).await;

    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let (ct, body) = multipart_body(&[
        ("title", None, b"Audio Title Series"),
        ("author", None, b"Audio Author"),
        ("series", None, b"The Analytical Engine"),
        ("series_index", None, b"3"),
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
    let book = db::get_book_by_uuid(&pool, &commit.uuid)
        .await
        .unwrap()
        .expect("uploaded audiobook should be indexed");
    assert_eq!(book.series.as_deref(), Some("The Analytical Engine"));
    assert_eq!(book.series_index.as_deref(), Some("3"));
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
async fn audiobook_inspect_rejects_unparseable_tags_with_415_bad_audio() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    // `M4B_MAGIC` passes `detect_audiobook_format`'s magic-byte gate (a valid
    // `ftyp` box header) but is too short to be a real MP4 container, so
    // lofty's tag parse fails — the only way to reach `UploadError::BadAudio`
    // rather than the earlier `UnsupportedAudioFormat` gate.
    let (ct, body) = multipart_body(&[("file", Some("stub.m4b"), M4B_MAGIC)]);
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

    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let msg = String::from_utf8_lossy(&bytes);
    assert!(
        msg.contains("could not read audiobook"),
        "expected the BadAudio message, got: {msg}"
    );
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
