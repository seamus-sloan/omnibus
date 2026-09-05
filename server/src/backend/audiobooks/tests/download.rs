//! `GET /api/audiobooks/{uuid}/download` and its per-part form: auth
//! gating, the unknown uuid and part, the download-permission 403s, and
//! the attachment served for the first or the requested part.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_audiobook_download_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/audiobooks/some-uuid/download"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_audiobook_download_returns_404_for_unknown_uuid() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/does-not-exist/download",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// AC1 (#1810): `can_download = 0` must 403 the attachment download route,
/// the same guard the OPDS acquisition delegate already enforces for
/// `/opds/audiobooks/{uuid}/download`.
#[tokio::test]
async fn api_get_audiobook_download_returns_403_when_user_cannot_download() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/some-uuid/download",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_get_audiobook_download_serves_first_part_as_attachment() {
    // Serves the lowest-ordinal part's real file, streamed via ServeFile,
    // with a forced attachment disposition suggesting the on-disk basename.
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let file_path = dir.path().join("Author/Book/01.mp3");
    let payload: Vec<u8> = (0u8..40).collect();
    std::fs::File::create(&file_path)
        .unwrap()
        .write_all(&payload)
        .unwrap();

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[
            (1, "Author/Book/02.mp3", 60.0),
            (0, "Author/Book/01.mp3", 60.0),
        ],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/download"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("audio/mpeg"),
    );
    let disposition = res
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        disposition.starts_with("attachment;"),
        "download must force an attachment disposition, got {disposition:?}"
    );
    // Lowest ordinal (0) wins even though it was inserted second.
    assert!(
        disposition.contains("01.mp3"),
        "should serve the lowest-ordinal part, got {disposition:?}"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), &payload[..]);
}

/// `?part=` is what lets an offline client take a whole multi-part book.
/// Without it the download route serves part 0 and nothing else, which is
/// what made a four-part audiobook look complete on the device after one
/// part.
#[tokio::test]
async fn api_get_audiobook_download_serves_the_requested_part_by_ordinal() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let second: Vec<u8> = (40u8..90).collect();
    for (name, bytes) in [
        ("Author/Book/01.mp3", vec![0u8; 10]),
        ("Author/Book/02.mp3", second.clone()),
    ] {
        std::fs::File::create(dir.path().join(name))
            .unwrap()
            .write_all(&bytes)
            .unwrap();
    }

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[
            (0, "Author/Book/01.mp3", 60.0),
            (1, "Author/Book/02.mp3", 60.0),
        ],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/download?part=1"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let disposition = res
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        disposition.contains("02.mp3"),
        "should serve the requested ordinal, got {disposition:?}"
    );
    let body = axum::body::to_bytes(res.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), &second[..]);
}

#[tokio::test]
async fn api_get_audiobook_download_returns_404_for_an_unknown_part_ordinal() {
    use std::io::Write;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    std::fs::File::create(dir.path().join("Author/Book/01.mp3"))
        .unwrap()
        .write_all(&[0u8; 10])
        .unwrap();

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_audiobook_with_parts(
        &pool,
        &library_path,
        "MP3",
        &[(0, "Author/Book/01.mp3", 60.0)],
    )
    .await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/audiobooks/{uuid}/download?part=7"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// The `can_download` gate is what `?part=` was added for — it must hold
/// on the per-part URL too, or planning an offline download against it
/// would hand a `can_download = 0` user the whole book.
#[tokio::test]
async fn api_get_audiobook_download_part_returns_403_when_user_cannot_download() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/audiobooks/some-uuid/download?part=1",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
