//! Conditional requests on the ebook byte routes — `ETag` revalidation,
//! `If-Range` resume, `If-Match`/`If-None-Match` precedence, and the 416 an
//! unsatisfiable range earns — plus the batch `POST
//! /api/downloads/validators` endpoint a device polls instead of asking
//! per book.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;
use super::content_disposition;

// --- Content validators (ETag / If-None-Match / If-Range) ---

/// Seed one EPUB book whose bytes really exist on disk, under a scratch dir
/// unique to this test run. Returns `(app, token, uuid, file_path, tmp)`;
/// the caller removes `tmp`.
async fn validator_fixture(
    label: &str,
) -> (
    axum::Router,
    String,
    String,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_validator_{label}_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let file_path = tmp.join("alpha.epub");
    std::fs::write(&file_path, EPUB_BYTES).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "44444444-4444-4444-4444-444444444444".to_string();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(&uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    (app, token, uuid, file_path, tmp)
}

const EPUB_BYTES: &[u8] = b"PK\x03\x04 fake-epub-with-a-body-long-enough-to-slice";

/// GET with a bearer token plus arbitrary extra headers.
fn get_with_headers(
    uri: &str,
    token: &str,
    extra: &[(axum::http::HeaderName, &str)],
) -> axum::http::Request<axum::body::Body> {
    let mut builder = axum::http::Request::builder()
        .uri(uri)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"));
    for (name, value) in extra {
        builder = builder.header(name, *value);
    }
    builder.body(axum::body::Body::empty()).unwrap()
}

fn etag_of(res: &axum::response::Response) -> String {
    res.headers()
        .get(axum::http::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string()
}

#[tokio::test]
async fn api_get_ebook_file_serves_an_etag_and_revalidates_with_304() {
    let (app, token, uuid, _path, tmp) = validator_fixture("file_304").await;
    let uri = format!("/api/ebooks/{uuid}/file");

    let first = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = etag_of(&first);
    assert!(!etag.is_empty(), "a 200 must publish a validator");
    // An ETag on an endpoint that authenticates by cookie *or* bearer needs a
    // Vary that names both, or a shared cache can hand one user's 304 to
    // another.
    assert_eq!(
        first
            .headers()
            .get(axum::http::header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some("Cookie, Authorization")
    );

    let revalidated = app
        .clone()
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, &etag)],
        ))
        .await
        .unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(etag_of(&revalidated), etag, "a 304 echoes the validator");

    let stale = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, "\"stale\"")],
        ))
        .await
        .unwrap();
    assert_eq!(stale.status(), StatusCode::OK);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_serves_a_partial_body_when_if_range_still_matches() {
    let (app, token, uuid, _path, tmp) = validator_fixture("file_resume_ok").await;
    let uri = format!("/api/ebooks/{uuid}/file");

    let etag = etag_of(
        &app.clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .unwrap(),
    );
    let res = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[
                (axum::http::header::IF_RANGE, &etag),
                (axum::http::header::RANGE, "bytes=4-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(
        &bytes[..],
        &EPUB_BYTES[4..],
        "resume continues the same file"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_serves_the_whole_new_body_when_if_range_went_stale() {
    // The splice this whole mechanism exists to prevent: a resume whose
    // validator no longer matches must never receive a 206 whose offsets
    // belong to the file that used to be here.
    let (app, token, uuid, _path, tmp) = validator_fixture("file_resume_stale").await;
    let uri = format!("/api/ebooks/{uuid}/file");

    let res = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[
                (axum::http::header::IF_RANGE, "\"the-previous-file\""),
                (axum::http::header::RANGE, "bytes=4-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], EPUB_BYTES, "a stale resume restarts from zero");

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_validator_moves_when_the_file_is_replaced() {
    let (app, token, uuid, path, tmp) = validator_fixture("file_replaced").await;
    let uri = format!("/api/ebooks/{uuid}/file");

    let before = etag_of(
        &app.clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .unwrap(),
    );
    std::fs::write(&path, b"PK\x03\x04 a-different-book-entirely-now-in-place").unwrap();
    let after = etag_of(&app.oneshot(get_with_bearer(&uri, &token)).await.unwrap());
    assert_ne!(before, after, "replacing the bytes must move the validator");

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_download_serves_an_etag_and_revalidates_with_304() {
    let (app, token, uuid, _path, tmp) = validator_fixture("download_304").await;
    let uri = format!("/api/ebooks/{uuid}/download");

    let first = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = etag_of(&first);
    assert!(!etag.is_empty());
    // The download keeps its attachment disposition — the validator rides
    // alongside it, not instead of it.
    assert!(content_disposition(&first).starts_with("attachment"));

    let revalidated = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[(axum::http::header::IF_NONE_MATCH, &etag)],
        ))
        .await
        .unwrap();
    assert_eq!(revalidated.status(), StatusCode::NOT_MODIFIED);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_download_serves_the_whole_new_body_when_if_range_went_stale() {
    let (app, token, uuid, _path, tmp) = validator_fixture("download_resume_stale").await;
    let uri = format!("/api/ebooks/{uuid}/download");

    let res = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[
                (axum::http::header::IF_RANGE, "\"the-previous-file\""),
                (axum::http::header::RANGE, "bytes=4-"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], EPUB_BYTES);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_answers_an_unsatisfiable_range_with_416_not_404() {
    // Reporting 416 as 404 tells a resuming client the book disappeared and
    // throws away the `Content-Range: bytes */<len>` it needs to restart.
    let (app, token, uuid, _path, tmp) = validator_fixture("file_416").await;

    let res = app
        .oneshot(get_with_headers(
            &format!("/api/ebooks/{uuid}/file"),
            &token,
            &[(
                axum::http::header::RANGE,
                &format!("bytes={}-", EPUB_BYTES.len() + 10),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_RANGE)
            .and_then(|v| v.to_str().ok()),
        Some(format!("bytes */{}", EPUB_BYTES.len()).as_str())
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_download_answers_an_unsatisfiable_range_with_416_not_404() {
    let (app, token, uuid, _path, tmp) = validator_fixture("download_416").await;

    let res = app
        .oneshot(get_with_headers(
            &format!("/api/ebooks/{uuid}/download"),
            &token,
            &[(
                axum::http::header::RANGE,
                &format!("bytes={}-", EPUB_BYTES.len() + 10),
            )],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::RANGE_NOT_SATISFIABLE);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_owes_the_full_body_when_if_none_match_is_stale_but_a_date_matches() {
    // RFC 9110 §13.2.2 precedence, at the endpoint: the date condition is
    // only consulted when `If-None-Match` is absent. Evaluating the ETag
    // condition in one layer and the date condition in another produced a
    // 304 here — handing back the very copy the client had just reported as
    // out of date.
    let (app, token, uuid, _path, tmp) = validator_fixture("file_precedence").await;
    let uri = format!("/api/ebooks/{uuid}/file");

    let first = app
        .clone()
        .oneshot(get_with_bearer(&uri, &token))
        .await
        .unwrap();
    let last_modified = first
        .headers()
        .get(axum::http::header::LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .expect("a 200 must publish Last-Modified to condition on")
        .to_string();

    let res = app
        .oneshot(get_with_headers(
            &uri,
            &token,
            &[
                (axum::http::header::IF_NONE_MATCH, "\"stale\""),
                (axum::http::header::IF_MODIFIED_SINCE, &last_modified),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_file_fails_a_non_matching_if_match_with_412() {
    let (app, token, uuid, _path, tmp) = validator_fixture("file_412").await;

    let res = app
        .oneshot(get_with_headers(
            &format!("/api/ebooks/{uuid}/file"),
            &token,
            &[(axum::http::header::IF_MATCH, "\"someone-elses\"")],
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PRECONDITION_FAILED);

    std::fs::remove_dir_all(&tmp).ok();
}

// --- POST /api/downloads/validators ---

fn post_json_with_bearer(
    uri: &str,
    token: &str,
    body: &str,
) -> axum::http::Request<axum::body::Body> {
    axum::http::Request::builder()
        .method("POST")
        .uri(uri)
        .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn api_post_download_validators_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/downloads/validators")
        .header(axum::http::header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(r#"{"files":[]}"#))
        .unwrap();
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_post_download_validators_answers_a_whole_device_in_one_request() {
    // The point of the endpoint: N downloads used to mean N full metadata
    // fetches on a timer. This is one request carrying no metadata at all.
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    for (uuid, mtime) in [("bk-1", 255), ("bk-2", 511)] {
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, scan_key, library_id, path, title) \
             VALUES (?, ?, 1, '/lib/b', 'B') RETURNING id",
        )
        .bind(uuid)
        .bind(uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
             VALUES (?, 'EPUB', 'b', 4096, ?)",
        )
        .bind(id)
        .bind(mtime)
        .execute(&pool)
        .await
        .unwrap();
    }

    let app = crate::backend::rest_router(AppState::new(pool));
    let body = r#"{"files":[
        {"book_uuid":"bk-1","format":"epub"},
        {"book_uuid":"bk-2","format":"epub"},
        {"book_uuid":"gone","format":"audio"}
    ]}"#;
    let res = app
        .oneshot(post_json_with_bearer(
            "/api/downloads/validators",
            &token,
            body,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let answer: omnibus_shared::DownloadValidatorResponse = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(answer.files.len(), 3);
    assert_eq!(answer.files[0].etag.as_deref(), Some("\"ff-1000\""));
    assert_eq!(answer.files[1].etag.as_deref(), Some("\"1ff-1000\""));
    assert_eq!(
        answer.files[2].etag, None,
        "an unanswerable file reads as can't-tell, not as unchanged"
    );
}

#[tokio::test]
async fn api_post_download_validators_rejects_an_oversized_batch() {
    // A device asks about what it has on disk; the cap only stops a
    // malformed or hostile body turning one request into unbounded work.
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));

    let files: Vec<String> = (0..omnibus_shared::MAX_VALIDATOR_QUERY + 1)
        .map(|i| format!(r#"{{"book_uuid":"bk-{i}","format":"epub"}}"#))
        .collect();
    let body = format!(r#"{{"files":[{}]}}"#, files.join(","));

    let res = app
        .oneshot(post_json_with_bearer(
            "/api/downloads/validators",
            &token,
            &body,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}
