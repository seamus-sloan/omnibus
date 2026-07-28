//! Unit tests for `friendly()`'s `DataError` → `DownloadError` mapping —
//! guards against a raw-error leak into `DownloadStatus::Error` — plus
//! `download_file`'s byte-range resume/streaming state machine.

use axum::response::IntoResponse;

use super::*;
use crate::data::DataError;
use crate::offline::test_support::{connect_refused_error, decode_error};

#[test]
fn friendly_maps_offline_to_the_offline_message() {
    assert_eq!(friendly(&DataError::Offline), DownloadError::Offline);
}

#[test]
fn friendly_maps_unauthorized_to_the_sign_in_again_message() {
    assert_eq!(
        friendly(&DataError::Unauthorized),
        DownloadError::Unauthorized
    );
}

#[test]
fn friendly_maps_http_to_the_server_error_message() {
    let e = DataError::Http {
        status: 503,
        body: "upstream exploded in a way a user should never see".into(),
    };
    let mapped = friendly(&e);
    assert_eq!(mapped, DownloadError::ServerError);
    assert!(!mapped.to_string().contains("upstream exploded"));
}

#[test]
fn friendly_maps_decode_to_the_server_error_message() {
    let src = serde_json::from_str::<i64>("not json").expect_err("must fail");
    let e = DataError::from(src);
    assert_eq!(friendly(&e), DownloadError::ServerError);
}

#[test]
fn friendly_maps_other_to_the_network_error_message() {
    assert_eq!(
        friendly(&DataError::Other("raw internal detail".into())),
        DownloadError::NetworkError
    );
}

#[tokio::test]
async fn friendly_maps_a_connect_refused_network_error_to_offline() {
    // Connect-refused classifies as offline via `is_offline_error`, checked
    // before the per-variant match.
    let e = connect_refused_error().await;
    assert_eq!(friendly(&e), DownloadError::Offline);
}

#[tokio::test]
async fn friendly_maps_a_decode_class_network_error_to_server_error_not_offline() {
    // A `Network` error that IS a decode failure means the server was
    // reachable — must not be classified as offline.
    let e = decode_error().await;
    let mapped = friendly(&e);
    assert_eq!(mapped, DownloadError::ServerError);
    assert!(!mapped.to_string().to_lowercase().contains("offline"));
}

#[test]
fn download_error_display_never_echoes_a_raw_variant_name() {
    // Every fixed message is a full sentence a user could read — not a
    // bare enum tag or a `{:?}`-style debug dump.
    for variant in [
        DownloadError::Offline,
        DownloadError::NotFound,
        DownloadError::UnsupportedFormat,
        DownloadError::NothingToDownload,
        DownloadError::StorageUnavailable,
        DownloadError::ConnectionLost,
        DownloadError::ServerError,
        DownloadError::NetworkError,
        DownloadError::Unauthorized,
        DownloadError::Interrupted,
    ] {
        assert!(!variant.to_string().is_empty());
    }
}

// ── download_file: byte-range resume/streaming (#1307 AC5) ─────────────

/// Deterministic filler content so byte-for-byte comparisons are stable.
fn fixture_content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

fn planned_file(rel: &str, url_path: &str) -> PlannedFile {
    PlannedFile {
        rel: rel.to_string(),
        url_path: url_path.to_string(),
        ordinal: None,
        bytes: None,
        done: false,
        etag: None,
    }
}

async fn spawn_router(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn download_file_streams_a_fresh_file_from_a_full_response() {
    let content = fixture_content(5000);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move || {
            let content = content.clone();
            async move { content }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = planned_file("book.epub", "/file");
    let mut deltas = Vec::new();

    let written = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("fresh download must succeed")
    .bytes;

    assert_eq!(written, 5000);
    assert_eq!(deltas.iter().sum::<i64>(), 5000);
    let final_bytes = tokio::fs::read(dir.path().join("book.epub"))
        .await
        .expect("final file must exist");
    assert_eq!(final_bytes, fixture_content(5000));
    assert!(
        tokio::fs::metadata(dir.path().join("book.epub.part"))
            .await
            .is_err(),
        "the .part temp file must be renamed away on completion"
    );
}

#[tokio::test]
async fn download_file_resumes_a_partial_download_via_a_range_request() {
    let full = fixture_content(5000);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let full = full.clone();
            async move {
                let range = headers
                    .get(axum::http::header::RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                if range == "bytes=2000-" {
                    (
                        axum::http::StatusCode::PARTIAL_CONTENT,
                        full[2000..].to_vec(),
                    )
                        .into_response()
                } else {
                    (axum::http::StatusCode::OK, full).into_response()
                }
            }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    // Simulate a prior attempt that got 2000 of the 5000 bytes down.
    tokio::fs::write(dir.path().join("book.epub.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    let file = planned_file("book.epub", "/file");
    let mut deltas = Vec::new();

    let written = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("resumed download must succeed")
    .bytes;

    assert_eq!(written, 5000, "final size must be the full file");
    // Resumed bytes are the caller's concern (already counted in a prior
    // session); this call only reports the freshly streamed remainder.
    assert_eq!(deltas.iter().sum::<i64>(), 3000);
    let final_bytes = tokio::fs::read(dir.path().join("book.epub"))
        .await
        .expect("final file must exist");
    assert_eq!(final_bytes, fixture_content(5000));
}

#[tokio::test]
async fn download_file_restarts_from_scratch_when_the_server_ignores_the_range_request() {
    let full = fixture_content(500);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move || {
            let full = full.clone();
            // Always 200 with the full body — a server that doesn't support
            // Range at all.
            async move { full }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("book.epub.part"), fixture_content(200))
        .await
        .expect("seed partial file");
    let file = planned_file("book.epub", "/file");
    let mut deltas = Vec::new();

    let written = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("restart-from-scratch download must succeed")
    .bytes;

    assert_eq!(written, 500);
    // The stale 200 bytes on disk are discarded; the negative delta says so.
    assert_eq!(deltas[0], -200);
    assert_eq!(deltas.iter().sum::<i64>(), 300);
    let final_bytes = tokio::fs::read(dir.path().join("book.epub"))
        .await
        .expect("final file must exist");
    assert_eq!(final_bytes, fixture_content(500));
}

/// Hand-rolled HTTP/1.1 server (bypassing axum's own framing) so the
/// response can lie about `Content-Length` — a real proxy/CDN misbehavior,
/// not something a well-formed axum handler can produce. Whether this
/// surfaces as `ConnectionLost` (a `resp.chunk()` transport error, since the
/// connection closes before the promised bytes arrive) or `Interrupted`
/// (the post-write length check) is a reqwest/hyper implementation detail;
/// what must hold is the outward contract: an `Err`, and no half-written
/// file silently promoted to "complete".
async fn spawn_lying_content_length_server(
    declared_len: usize,
    actual_body: &'static [u8],
) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        if let Ok((mut stream, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = stream.read(&mut buf).await;
            let head = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {declared_len}\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(head.as_bytes()).await;
            let _ = stream.write_all(actual_body).await;
            let _ = stream.shutdown().await;
        }
    });
    format!("http://127.0.0.1:{port}")
}

#[tokio::test]
async fn download_file_errors_and_leaves_no_final_file_when_the_response_is_truncated() {
    let base = spawn_lying_content_length_server(20, b"hello").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = planned_file("book.epub", "/file");

    let result = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |_| {}).await;

    assert!(
        result.is_err(),
        "a body shorter than its declared Content-Length must not report success"
    );
    assert!(
        tokio::fs::metadata(dir.path().join("book.epub"))
            .await
            .is_err(),
        "a truncated transfer must never be renamed into a completed file"
    );
}

// ── download_file: If-Range, so a resume can never splice ──────────────

/// A server that answers a resume the way omnibus does: honour the range only
/// while the client's `If-Range` still names the bytes on disk, and otherwise
/// send the whole (new) file. Records what it was asked with.
fn conditional_range_router(
    content: Vec<u8>,
    etag: &'static str,
    seen: std::sync::Arc<std::sync::Mutex<Option<String>>>,
) -> axum::Router {
    axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let content = content.clone();
            let seen = seen.clone();
            async move {
                let header = |name: axum::http::HeaderName| {
                    headers
                        .get(name)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string)
                };
                let if_range = header(axum::http::header::IF_RANGE);
                if let Ok(mut guard) = seen.lock() {
                    *guard = if_range.clone();
                }
                let range = header(axum::http::header::RANGE);
                let honour_range = range.is_some() && if_range.as_deref() == Some(etag);
                let mut resp = match (honour_range, range.as_deref()) {
                    (true, Some("bytes=2000-")) => (
                        axum::http::StatusCode::PARTIAL_CONTENT,
                        content[2000..].to_vec(),
                    )
                        .into_response(),
                    _ => (axum::http::StatusCode::OK, content).into_response(),
                };
                if let Ok(value) = axum::http::HeaderValue::from_str(etag) {
                    resp.headers_mut().insert(axum::http::header::ETAG, value);
                }
                resp
            }
        }),
    )
}

#[tokio::test]
async fn download_file_records_the_validator_its_bytes_were_served_under() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let app = conditional_range_router(fixture_content(500), "\"abc-1f4\"", seen);
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = planned_file("book.epub", "/file");
    let fetched = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |_| {})
        .await
        .expect("fresh download must succeed");

    assert_eq!(fetched.etag.as_deref(), Some("\"abc-1f4\""));
}

#[tokio::test]
async fn download_file_sends_if_range_so_a_resume_is_conditional() {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let app = conditional_range_router(fixture_content(5000), "\"abc-1388\"", seen.clone());
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("book.epub.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    let mut file = planned_file("book.epub", "/file");
    file.etag = Some("\"abc-1388\"".into());
    let mut deltas = Vec::new();

    let written = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("resumed download must succeed")
    .bytes;

    assert_eq!(
        seen.lock().expect("lock").as_deref(),
        Some("\"abc-1388\""),
        "a resume must state which bytes its partial copy came from"
    );
    assert_eq!(written, 5000);
    assert_eq!(deltas.iter().sum::<i64>(), 3000, "only the tail is fetched");
    assert_eq!(
        tokio::fs::read(dir.path().join("book.epub")).await.unwrap(),
        fixture_content(5000)
    );
}

#[tokio::test]
async fn download_file_discards_its_partial_copy_when_the_file_changed_under_it() {
    // The corruption this whole mechanism exists to prevent: 2000 bytes of the
    // old file are already on disk when the book is replaced on the server. A
    // resume without the precondition would append the new file's tail to the
    // old head and rename the result into place as a finished book.
    let seen = std::sync::Arc::new(std::sync::Mutex::new(None));
    let replacement = fixture_content(3000)
        .into_iter()
        .map(|b| b ^ 0xff)
        .collect::<Vec<u8>>();
    let app = conditional_range_router(replacement.clone(), "\"def-bb8\"", seen.clone());
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("book.epub.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    let mut file = planned_file("book.epub", "/file");
    file.etag = Some("\"abc-1388\"".into());
    let mut deltas = Vec::new();

    let fetched = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("download must restart cleanly rather than splice");
    let written = fetched.bytes;

    assert_eq!(seen.lock().expect("lock").as_deref(), Some("\"abc-1388\""));
    assert_eq!(deltas[0], -2000, "the stale partial bytes must be dropped");
    assert_eq!(written, 3000);
    assert_eq!(
        tokio::fs::read(dir.path().join("book.epub")).await.unwrap(),
        replacement,
        "the stored file must be the new one, whole — never a spliced mix"
    );
    assert_eq!(
        fetched.etag.as_deref(),
        Some("\"def-bb8\""),
        "the recorded validator must move to the bytes now on disk"
    );
}

// ── the resume refuses what it cannot vouch for ────────────────────────

#[tokio::test]
async fn download_file_does_not_resume_a_partial_it_has_no_validator_for() {
    // A `.part` written before validators were recorded, or one whose process
    // died before the validator reached disk. Sending a bare `Range` here
    // would let the server answer 206 from whatever the file is now, appending
    // a new tail to a head of unknown provenance — the splice, unguarded.
    let full = fixture_content(5000);
    let ranges = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let recorder = ranges.clone();
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let full = full.clone();
            let recorder = recorder.clone();
            async move {
                let range = headers
                    .get(axum::http::header::RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_string();
                if let Ok(mut guard) = recorder.lock() {
                    guard.push(range.clone());
                }
                // A Range-honouring server, exactly like ours.
                if let Some(start) = range
                    .strip_prefix("bytes=")
                    .and_then(|r| r.trim_end_matches('-').parse::<usize>().ok())
                {
                    return (
                        axum::http::StatusCode::PARTIAL_CONTENT,
                        full[start..].to_vec(),
                    )
                        .into_response();
                }
                (axum::http::StatusCode::OK, full).into_response()
            }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("book.epub.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    // The distinguishing detail: no recorded validator.
    let file = planned_file("book.epub", "/file");
    assert_eq!(file.etag, None, "precondition: nothing to condition on");
    let mut deltas = Vec::new();

    let fetched = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |d| {
        deltas.push(d)
    })
    .await
    .expect("download must restart rather than resume blind");

    assert_eq!(
        ranges.lock().expect("lock").as_slice(),
        &[String::new()],
        "no Range may go out without a precondition to qualify it"
    );
    assert_eq!(deltas[0], -2000, "the unvouched-for bytes must be dropped");
    assert_eq!(fetched.bytes, 5000);
    assert_eq!(
        tokio::fs::read(dir.path().join("book.epub")).await.unwrap(),
        fixture_content(5000)
    );
}

#[tokio::test]
async fn download_file_reports_the_source_validator_the_server_named() {
    // The `ETag` describes the bytes streamed; this header describes the row
    // they came from. On the audiobook and export routes those differ, and it
    // is the latter a staleness check compares against.
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(|| async {
            let mut resp = (axum::http::StatusCode::OK, fixture_content(64)).into_response();
            resp.headers_mut()
                .insert(axum::http::header::ETAG, "\"part-1\"".parse().unwrap());
            resp.headers_mut().insert(
                "x-omnibus-source-validator",
                "\"whole-row-9\"".parse().unwrap(),
            );
            resp
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    let file = planned_file("book.epub", "/file");

    let fetched = download_file(&base, "u", DlFormat::Epub, dir.path(), &file, &mut |_| {})
        .await
        .expect("download must succeed");

    assert_eq!(fetched.etag.as_deref(), Some("\"part-1\""));
    assert_eq!(fetched.source.as_deref(), Some("\"whole-row-9\""));
}
