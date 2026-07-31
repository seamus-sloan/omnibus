//! Unit tests for `friendly()`'s `DataError` → `DownloadError` mapping —
//! guards against a raw-error leak into `DownloadStatus::Error` — plus
//! `download_file`'s byte-range resume/streaming state machine.

use axum::response::IntoResponse;

use super::*;
use crate::data::DataError;
use crate::offline::test_support::{connect_refused_error, decode_error, spawn_router};

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
        DownloadError::Damaged,
    ] {
        assert!(!variant.to_string().is_empty());
    }
}

// ── download_file: byte-range resume/streaming (#1307 AC5) ─────────────

/// Deterministic filler content so byte-for-byte comparisons are stable.
fn fixture_content(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

/// A planned file for the transfer-mechanics tests below.
///
/// `rel` is deliberately an `.mp3`: those tests stream synthetic filler
/// bytes, and raw MP3 is the one format with no container for
/// `verify::verify` to check, so they exercise streaming and resume
/// without also having to be a well-formed archive. Container
/// verification has its own tests in `downloads/verify/tests.rs`, and its
/// integration with this loop is covered by
/// `download_file_rejects_and_removes_a_file_that_fails_verification`.
fn planned_file(rel: &str, url_path: &str) -> PlannedFile {
    PlannedFile {
        rel: rel.to_string(),
        url_path: url_path.to_string(),
        ordinal: None,
        bytes: None,
        done: false,
        http_etag: None,
        source_etag: None,
    }
}

/// `download_file` with the observed-ETag out-param discarded, for the
/// transfer-mechanics tests that don't assert on it.
async fn download_file_ignoring_etag(
    base: &str,
    dir: &std::path::Path,
    file: &PlannedFile,
    on_delta: &mut (dyn FnMut(i64) + Send),
) -> anyhow::Result<Fetched> {
    download_file(base, dir, file, &mut |_| {}, on_delta).await
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
    let file = planned_file("part-0.mp3", "/file");
    let mut deltas = Vec::new();

    let fetched = download_file_ignoring_etag(&base, dir.path(), &file, &mut |d| deltas.push(d))
        .await
        .expect("fresh download must succeed");

    assert_eq!(fetched.bytes, 5000);
    assert_eq!(deltas.iter().sum::<i64>(), 5000);
    let final_bytes = tokio::fs::read(dir.path().join("part-0.mp3"))
        .await
        .expect("final file must exist");
    assert_eq!(final_bytes, fixture_content(5000));
    assert!(
        tokio::fs::metadata(dir.path().join("part-0.mp3.part"))
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
    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    let file = planned_file("part-0.mp3", "/file");
    let mut deltas = Vec::new();

    let fetched = download_file_ignoring_etag(&base, dir.path(), &file, &mut |d| deltas.push(d))
        .await
        .expect("resumed download must succeed");

    assert_eq!(fetched.bytes, 5000, "final size must be the full file");
    // Resumed bytes are the caller's concern (already counted in a prior
    // session); this call only reports the freshly streamed remainder.
    assert_eq!(deltas.iter().sum::<i64>(), 3000);
    let final_bytes = tokio::fs::read(dir.path().join("part-0.mp3"))
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
    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(200))
        .await
        .expect("seed partial file");
    let file = planned_file("part-0.mp3", "/file");
    let mut deltas = Vec::new();

    let fetched = download_file_ignoring_etag(&base, dir.path(), &file, &mut |d| deltas.push(d))
        .await
        .expect("restart-from-scratch download must succeed");

    assert_eq!(fetched.bytes, 500);
    // The stale 200 bytes on disk are discarded; the negative delta says so.
    assert_eq!(deltas[0], -200);
    assert_eq!(deltas.iter().sum::<i64>(), 300);
    let final_bytes = tokio::fs::read(dir.path().join("part-0.mp3"))
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
                "HTTP/1.1 200 OK\r\nContent-Length: {declared_len}\r\n\
                 ETag: \"mid-transfer\"\r\nConnection: close\r\n\r\n"
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
    let file = planned_file("part-0.mp3", "/file");

    let result = download_file_ignoring_etag(&base, dir.path(), &file, &mut |_| {}).await;

    assert!(
        result.is_err(),
        "a body shorter than its declared Content-Length must not report success"
    );
    assert!(
        tokio::fs::metadata(dir.path().join("part-0.mp3"))
            .await
            .is_err(),
        "a truncated transfer must never be renamed into a completed file"
    );
}

// ── Validator-aware resume + integrity (#1231) ─────────────────────────

/// A well-formed EPUB, so a download that lands intact passes `verify` the
/// way a real one would — CRC included, which the check actually reads.
fn valid_epub_bytes() -> Vec<u8> {
    crate::offline::test_support::zip_one(b"mimetype", b"application/epub+zip")
}

#[tokio::test]
async fn download_file_sends_if_range_with_the_stored_validator_when_resuming() {
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let recorder = seen.clone();
    let full = fixture_content(5000);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let full = full.clone();
            let recorder = recorder.clone();
            async move {
                *recorder.lock().expect("lock") = headers
                    .get(axum::http::header::IF_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_string);
                (
                    axum::http::StatusCode::PARTIAL_CONTENT,
                    full[2000..].to_vec(),
                )
                    .into_response()
            }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(2000))
        .await
        .expect("seed partial file");
    let mut file = planned_file("part-0.mp3", "/file");
    file.http_etag = Some("\"abc-123\"".into());

    download_file_ignoring_etag(&base, dir.path(), &file, &mut |_| {})
        .await
        .expect("resumed download must succeed");

    assert_eq!(
        seen.lock().expect("lock").as_deref(),
        Some("\"abc-123\""),
        "a resume must ask the server to confirm the file has not moved"
    );
}

#[tokio::test]
async fn download_file_omits_if_range_when_no_validator_was_stored() {
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let recorder = seen.clone();
    let full = fixture_content(500);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let full = full.clone();
            let recorder = recorder.clone();
            async move {
                *recorder.lock().expect("lock") =
                    headers.contains_key(axum::http::header::IF_RANGE);
                full
            }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(200))
        .await
        .expect("seed partial file");

    download_file_ignoring_etag(
        &base,
        dir.path(),
        &planned_file("part-0.mp3", "/file"),
        &mut |_| {},
    )
    .await
    .expect("download must succeed");

    assert!(
        !*seen.lock().expect("lock"),
        "an empty If-Range would be malformed; send no header at all"
    );
}

#[tokio::test]
async fn download_file_records_the_response_validator_for_the_next_resume() {
    let content = valid_epub_bytes();
    let app =
        axum::Router::new().route(
            "/file",
            axum::routing::get(move || {
                let content = content.clone();
                async move {
                    ([(axum::http::header::ETAG, "\"deadbeef-1000\"")], content).into_response()
                }
            }),
        );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");

    let mut observed = None;
    download_file(
        &base,
        dir.path(),
        &planned_file("book.epub", "/file"),
        &mut |etag| observed = etag,
        &mut |_| {},
    )
    .await
    .expect("download must succeed");

    assert_eq!(observed.as_deref(), Some("\"deadbeef-1000\""));
}

#[tokio::test]
async fn download_file_rejects_and_removes_a_file_that_fails_verification() {
    // The bytes arrive complete and the length checks out — only the
    // container structure gives away that this is not a book. Without the
    // verification step this would be promoted to a "complete" download and
    // sit on disk failing to open.
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(|| async { b"<html>504 Gateway Timeout</html>".to_vec() }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");

    let result = download_file_ignoring_etag(
        &base,
        dir.path(),
        &planned_file("book.epub", "/file"),
        &mut |_| {},
    )
    .await;

    assert!(
        result.is_err(),
        "a damaged container must not report success"
    );
    assert!(
        tokio::fs::metadata(dir.path().join("book.epub"))
            .await
            .is_err(),
        "the damaged file must be removed, not left for a resume to build on"
    );
}

#[tokio::test]
async fn download_file_accepts_a_structurally_valid_container() {
    let content = valid_epub_bytes();
    let expected = content.clone();
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move || {
            let content = content.clone();
            async move { content }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");

    let fetched = download_file_ignoring_etag(
        &base,
        dir.path(),
        &planned_file("book.epub", "/file"),
        &mut |_| {},
    )
    .await
    .expect("a well-formed archive must pass verification");

    assert_eq!(fetched.bytes, expected.len() as i64);
    assert_eq!(
        tokio::fs::read(dir.path().join("book.epub")).await.unwrap(),
        expected
    );
}

#[tokio::test]
async fn the_validator_is_handed_over_before_any_body_byte_is_read() {
    // Durability, not just error-path coverage. A process kill or a dropped
    // task runs no code after the transfer, so a tag written when
    // `download_file` returns never reaches disk for exactly the
    // interruptions a phone actually suffers — and the `.part` they leave
    // resumes with a bare `Range`.
    //
    // The invariant that makes the tag durable is *ordering*: the callback
    // must fire before the first byte is streamed, so whatever the caller
    // does with it has already happened by the time a kill could land.
    let content = fixture_content(5000);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move || {
            let content = content.clone();
            async move { ([(axum::http::header::ETAG, "\"live\"")], content).into_response() }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");

    // Both closures append to one log, so the order they ran in is the
    // assertion.
    let events = std::sync::Mutex::new(Vec::<&'static str>::new());
    download_file(
        &base,
        dir.path(),
        &planned_file("part-0.mp3", "/file"),
        &mut |etag| {
            assert_eq!(etag.as_deref(), Some("\"live\""));
            events.lock().expect("lock").push("etag");
        },
        &mut |_| events.lock().expect("lock").push("bytes"),
    )
    .await
    .expect("download");

    let events = events.into_inner().expect("lock");
    assert_eq!(
        events.first(),
        Some(&"etag"),
        "the validator must be handed over before the first streamed byte"
    );
    assert!(events.contains(&"bytes"), "the body did stream afterwards");
}

#[test]
fn record_file_etag_updates_the_registry_entry_in_place() {
    let uuid = "u-record-etag";
    crate::offline::downloads::upsert(DownloadEntry {
        book_uuid: uuid.into(),
        format: DlFormat::Epub,
        title: "T".into(),
        file_id: None,
        status: DownloadStatus::Downloading {
            downloaded: 0,
            total: None,
        },
        files: vec![planned_file("book.epub", "/file")],
        updated_at: 1,
        stale: false,
    });

    crate::offline::downloads::record_file_etag(
        uuid,
        DlFormat::Epub,
        "book.epub",
        Some("\"v1\"".into()),
    );

    let entry = crate::offline::downloads::get_entry(uuid, DlFormat::Epub).expect("entry");
    assert_eq!(entry.files[0].http_etag.as_deref(), Some("\"v1\""));
}

#[tokio::test]
async fn download_file_records_the_validator_even_when_the_transfer_then_fails() {
    // The lifecycle the manually-seeded `http_etag` tests missed: the tag
    // has to be persisted from the *response headers*, not from a
    // successful result, or the one case that needs it — resuming the
    // `.part` an interrupted transfer left behind — never has it and falls
    // back to a bare `Range`.
    let base = spawn_lying_content_length_server(5000, b"short").await;
    let dir = tempfile::tempdir().expect("tempdir");
    let mut observed = None;

    let result = download_file(
        &base,
        dir.path(),
        &planned_file("part-0.mp3", "/file"),
        &mut |etag| observed = etag,
        &mut |_| {},
    )
    .await;

    assert!(
        result.is_err(),
        "a truncated transfer must not report success"
    );
    assert_eq!(
        observed.as_deref(),
        Some("\"mid-transfer\""),
        "the validator must survive the failure that leaves a .part behind"
    );
}

#[tokio::test]
async fn a_resume_after_an_interrupted_transfer_offers_the_recorded_validator() {
    // End to end over the two attempts, driving the registry the way the
    // engine does rather than hand-seeding it.
    use std::sync::{Arc, Mutex};

    let seen: Arc<Mutex<Vec<Option<String>>>> = Arc::new(Mutex::new(Vec::new()));
    let recorder = seen.clone();
    let full = fixture_content(5000);
    let app = axum::Router::new().route(
        "/file",
        axum::routing::get(move |headers: axum::http::HeaderMap| {
            let full = full.clone();
            let recorder = recorder.clone();
            async move {
                recorder.lock().expect("lock").push(
                    headers
                        .get(axum::http::header::IF_RANGE)
                        .and_then(|v| v.to_str().ok())
                        .map(str::to_string),
                );
                (
                    axum::http::StatusCode::PARTIAL_CONTENT,
                    [(axum::http::header::ETAG, "\"v1\"")],
                    full[2000..].to_vec(),
                )
                    .into_response()
            }
        }),
    );
    let base = spawn_router(app).await;
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(2000))
        .await
        .expect("seed the bytes an interrupted attempt left");

    // Attempt 1 records the tag; attempt 2 must replay it.
    let mut file = planned_file("part-0.mp3", "/file");
    let mut observed = None;
    download_file(
        &base,
        dir.path(),
        &file,
        &mut |etag| observed = etag,
        &mut |_| {},
    )
    .await
    .expect("download");
    file.http_etag = observed;

    tokio::fs::write(dir.path().join("part-0.mp3.part"), fixture_content(2000))
        .await
        .expect("seed a second partial");
    download_file(&base, dir.path(), &file, &mut |_| {}, &mut |_| {})
        .await
        .expect("download");

    let asked = seen.lock().expect("lock").clone();
    assert_eq!(asked.len(), 2);
    assert_eq!(
        asked[1].as_deref(),
        Some("\"v1\""),
        "the second attempt must ask the server to confirm the file hasn't moved"
    );
}

#[test]
fn provenance_only_claims_what_the_validators_prove() {
    assert_eq!(
        provenance(Some("\"same\""), Some("\"same\"")),
        Provenance::Proven
    );
    assert_eq!(
        provenance(Some("\"old\""), Some("\"new\"")),
        Provenance::Superseded
    );
    for pair in [
        (None, Some("\"new\"")),
        (Some("\"old\""), None),
        (None, None),
    ] {
        assert_eq!(provenance(pair.0, pair.1), Provenance::Unknown);
    }
}

#[test]
fn an_unprovable_part_is_only_reused_while_nothing_is_known() {
    assert!(reusable(Some("\"same\""), Some("\"same\"")));
    assert!(!reusable(Some("\"old\""), Some("\"new\"")));
    // A part written before validators existed, against a file that now has
    // one. Reusing it would restamp it with that validator — laundering
    // "no idea where these bytes came from" into "these bytes are current"
    // and letting a legacy part share an audiobook with fresh ones.
    assert!(!reusable(None, Some("\"new\"")));
    // Nothing is known on either side, so nothing has been learned and
    // refusing to resume would just break downloads for a row the scanner
    // has never stat'd.
    assert!(reusable(None, None));
    assert!(reusable(Some("\"old\""), None));
}

#[tokio::test]
async fn discard_partial_drops_the_partial_but_keeps_the_usable_copy() {
    // The partial has to go — resuming onto bytes from another edition is
    // the splice. The finished file must stay: it is about to be replaced
    // by an atomic rename, and until that lands it is the only copy the
    // reader has. Deleting it early turns any failure into a book that used
    // to work and now doesn't.
    let dir = tempfile::tempdir().expect("tempdir");
    tokio::fs::write(dir.path().join("part-0.m4b"), b"old-edition")
        .await
        .expect("write");
    tokio::fs::write(dir.path().join("part-0.m4b.part"), b"partial")
        .await
        .expect("write");

    discard_partial(dir.path(), "part-0.m4b").await;

    assert!(tokio::fs::metadata(dir.path().join("part-0.m4b.part"))
        .await
        .is_err());
    assert_eq!(
        tokio::fs::read(dir.path().join("part-0.m4b"))
            .await
            .unwrap(),
        b"old-edition"
    );
}
