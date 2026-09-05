//! The content validators on the byte-serving routes: the strong `ETag`
//! and its 304 revalidation, `If-Range` resuming while it matches and
//! serving the whole new body once stale, and the validator moving when a
//! part or segment is replaced on disk.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use tower::ServiceExt;

use super::super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

const PART_BYTES: &[u8] = b"ID3 a-stand-in-audio-part-long-enough-to-slice-in-half";

/// Seed one direct-play MP3 audiobook whose part really exists on disk.
/// Returns `(app, token, uuid, part_path, dir)` — `dir` keeps the temp tree
/// alive for the caller's lifetime.
async fn audio_validator_fixture() -> (
    axum::Router,
    String,
    String,
    std::path::PathBuf,
    tempfile::TempDir,
) {
    use std::io::Write;

    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let part_path = dir.path().join("Author/Book/01.mp3");
    std::fs::File::create(&part_path)
        .unwrap()
        .write_all(PART_BYTES)
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
    (app, token, uuid, part_path, dir)
}

fn audio_get(uri: &str, token: &str, extra: &[(axum::http::HeaderName, &str)]) -> Request<Body> {
    let mut builder = Request::builder()
        .uri(uri)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    for (name, value) in extra {
        builder = builder.header(name, *value);
    }
    builder.body(Body::empty()).unwrap()
}

fn audio_etag(res: &axum::response::Response) -> String {
    res.headers()
        .get(axum::http::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn api_get_audiobook_part_serves_an_etag_and_revalidates_with_304() {
    let (app, token, uuid, _path, _dir) = audio_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/parts/0");

    let first = app
        .clone()
        .oneshot(audio_get(&uri, &token, &[]))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = audio_etag(&first);
    assert!(!etag.is_empty(), "a 200 must publish a validator");
    assert_eq!(
        first
            .headers()
            .get(axum::http::header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some("Cookie, Authorization")
    );

    let revalidated = app
        .oneshot(audio_get(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, &etag)],
        ))
        .await
        .unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(audio_etag(&revalidated), etag);
}

#[tokio::test]
async fn api_get_audiobook_part_serves_the_whole_new_body_when_if_range_went_stale() {
    // A part replaced mid-download must restart, never resume into the
    // middle of a file that is no longer there.
    let (app, token, uuid, _path, _dir) = audio_validator_fixture().await;
    let res = app
        .oneshot(audio_get(
            &format!("/api/audiobooks/{uuid}/parts/0"),
            &token,
            &[
                (axum::http::header::IF_RANGE, "\"the-previous-file\""),
                (axum::http::header::RANGE, "bytes=8-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), PART_BYTES);
}

#[tokio::test]
async fn api_get_audiobook_part_resumes_when_if_range_still_matches() {
    let (app, token, uuid, _path, _dir) = audio_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/parts/0");
    let etag = audio_etag(
        &app.clone()
            .oneshot(audio_get(&uri, &token, &[]))
            .await
            .unwrap(),
    );

    let res = app
        .oneshot(audio_get(
            &uri,
            &token,
            &[
                (axum::http::header::IF_RANGE, &etag),
                (axum::http::header::RANGE, "bytes=8-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), &PART_BYTES[8..]);
}

#[tokio::test]
async fn api_get_audiobook_part_validator_moves_when_the_part_is_replaced() {
    let (app, token, uuid, path, _dir) = audio_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/parts/0");

    let before = audio_etag(
        &app.clone()
            .oneshot(audio_get(&uri, &token, &[]))
            .await
            .unwrap(),
    );
    std::fs::write(
        &path,
        b"ID3 a-completely-different-recording-in-the-same-slot",
    )
    .unwrap();
    let after = audio_etag(&app.oneshot(audio_get(&uri, &token, &[])).await.unwrap());
    assert_ne!(before, after);
}

#[tokio::test]
async fn api_get_audiobook_download_serves_an_etag_and_revalidates_with_304() {
    let (app, token, uuid, _path, _dir) = audio_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/download");

    let first = app
        .clone()
        .oneshot(audio_get(&uri, &token, &[]))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = audio_etag(&first);
    assert!(!etag.is_empty());

    let revalidated = app
        .oneshot(audio_get(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, &etag)],
        ))
        .await
        .unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn api_get_audiobook_download_serves_the_whole_new_body_when_if_range_went_stale() {
    let (app, token, uuid, _path, _dir) = audio_validator_fixture().await;
    let res = app
        .oneshot(audio_get(
            &format!("/api/audiobooks/{uuid}/download"),
            &token,
            &[
                (axum::http::header::IF_RANGE, "\"the-previous-file\""),
                (axum::http::header::RANGE, "bytes=8-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), PART_BYTES);
}

const SEGMENT_BYTES: &[u8] = b"a-stand-in-mpegts-segment-long-enough-to-slice-in-half";

/// Seed one direct-play audiobook and pre-write its first HLS segment
/// straight into the cache dir (bypassing the transcoder), so
/// `get_audiobook_segment` takes the fast "already on disk" path. Points
/// `OMNIBUS_DATA_DIR` at a fresh scratch dir via [`DataDirGuard`], which
/// holds the shared env lock so this can't race the status-handler tests'
/// own `OMNIBUS_DATA_DIR` swap. Returns `(app, token, uuid, segment_path,
/// data_dir)`; `data_dir` must stay alive for the caller's lifetime.
async fn segment_validator_fixture() -> (
    axum::Router,
    String,
    String,
    std::path::PathBuf,
    DataDirGuard,
) {
    use std::io::Write;

    let data_dir = DataDirGuard::new("audiobook_segment");

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "bob").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid =
        seed_audiobook_with_parts(&pool, "/audiobooks", "MP3", &[(0, "ch01.mp3", 60.0)]).await;
    let resolved = hls::resolve_audiobook(&pool, &uuid).await.unwrap().unwrap();

    let seg_dir = hls::segment_dir(resolved.book_id, hls::AUDIO64);
    std::fs::create_dir_all(&seg_dir).unwrap();
    let seg_path = seg_dir.join("seg-0000.ts");
    std::fs::File::create(&seg_path)
        .unwrap()
        .write_all(SEGMENT_BYTES)
        .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    (app, token, uuid, seg_path, data_dir)
}

#[tokio::test]
async fn api_get_audiobook_segment_serves_an_etag_and_revalidates_with_304() {
    let (app, token, uuid, _path, _dir) = segment_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/segments/seg-0000.ts");

    let first = app
        .clone()
        .oneshot(audio_get(&uri, &token, &[]))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    assert_eq!(
        first
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(MPEGTS_CONTENT_TYPE)
    );
    let etag = audio_etag(&first);
    assert!(!etag.is_empty(), "a 200 must publish a validator");
    let body = to_bytes(first.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), SEGMENT_BYTES);

    let revalidated = app
        .oneshot(audio_get(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, &etag)],
        ))
        .await
        .unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(audio_etag(&revalidated), etag);
}

#[tokio::test]
async fn api_get_audiobook_segment_resumes_when_if_range_still_matches() {
    let (app, token, uuid, _path, _dir) = segment_validator_fixture().await;
    let uri = format!("/api/audiobooks/{uuid}/segments/seg-0000.ts");
    let etag = audio_etag(
        &app.clone()
            .oneshot(audio_get(&uri, &token, &[]))
            .await
            .unwrap(),
    );

    let res = app
        .oneshot(audio_get(
            &uri,
            &token,
            &[
                (axum::http::header::IF_RANGE, &etag),
                (axum::http::header::RANGE, "bytes=8-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), &SEGMENT_BYTES[8..]);
}

#[tokio::test]
async fn api_get_audiobook_segment_serves_the_whole_new_body_when_if_range_went_stale() {
    let (app, token, uuid, _path, _dir) = segment_validator_fixture().await;
    let res = app
        .oneshot(audio_get(
            &format!("/api/audiobooks/{uuid}/segments/seg-0000.ts"),
            &token,
            &[
                (axum::http::header::IF_RANGE, "\"the-previous-segment\""),
                (axum::http::header::RANGE, "bytes=8-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), SEGMENT_BYTES);
}
