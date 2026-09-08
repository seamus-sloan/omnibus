//! The validator sweep against `POST /api/downloads/validators`, which
//! the server caps at `MAX_VALIDATOR_QUERY`: one request under the cap,
//! two for one entry over it, answers applied from every chunk, a failed
//! chunk keeping earlier answers but not marking the sweep done, and the
//! refresh timestamp advancing only after a full success.

#![allow(clippy::await_holding_lock)]

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use axum::response::IntoResponse;

use super::super::*;
use super::seed_complete_download;
use crate::offline::test_support::spawn_router;

// The server caps one `POST /api/downloads/validators` request at
// `MAX_VALIDATOR_QUERY` (422 above it). A device with more completed
// downloads than that must split the sweep into multiple requests, none
// over the cap, and must not record the sweep as done unless every chunk
// answered.
fn validator_query(uuid: &str) -> omnibus_shared::DownloadValidatorQuery {
    omnibus_shared::DownloadValidatorQuery {
        book_uuid: uuid.into(),
        format: omnibus_shared::DownloadFormat::Epub,
        file_id: None,
    }
}

/// A `/validators` mock that records each request's file count and answers
/// every query "can't tell" (`etag: None`) — enough for tests that only care
/// how the client splits the batch, not the answer content.
async fn spawn_counting_validator_server() -> (String, Arc<Mutex<Vec<usize>>>) {
    let sizes: Arc<Mutex<Vec<usize>>> = Arc::new(Mutex::new(Vec::new()));
    let recorded = sizes.clone();
    let app = axum::Router::new().route(
        "/api/downloads/validators",
        axum::routing::post(
            move |axum::Json(req): axum::Json<omnibus_shared::DownloadValidatorRequest>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().expect("lock").push(req.files.len());
                    axum::Json(omnibus_shared::DownloadValidatorResponse { files: vec![] })
                }
            },
        ),
    );
    let base = spawn_router(app).await;
    (base, sizes)
}

#[tokio::test]
async fn sweep_validators_sends_exactly_one_request_for_the_500_entry_limit() {
    let (base, sizes) = spawn_counting_validator_server().await;
    let queries: Vec<_> = (0..omnibus_shared::MAX_VALIDATOR_QUERY)
        .map(|i| validator_query(&format!("u-cap-{i}")))
        .collect();

    let ok = sweep_validators(&base, &queries).await;

    assert!(ok, "a batch at the cap must succeed");
    assert_eq!(
        sizes.lock().expect("lock").clone(),
        vec![omnibus_shared::MAX_VALIDATOR_QUERY],
        "exactly 500 entries must fit in one request"
    );
}

#[tokio::test]
async fn sweep_validators_splits_501_entries_into_two_requests_neither_over_the_limit() {
    let (base, sizes) = spawn_counting_validator_server().await;
    let queries: Vec<_> = (0..omnibus_shared::MAX_VALIDATOR_QUERY + 1)
        .map(|i| validator_query(&format!("u-over-{i}")))
        .collect();

    let ok = sweep_validators(&base, &queries).await;

    assert!(ok);
    let sent = sizes.lock().expect("lock").clone();
    assert_eq!(sent.len(), 2, "501 entries must take two requests");
    assert!(
        sent.iter()
            .all(|&n| n <= omnibus_shared::MAX_VALIDATOR_QUERY),
        "neither request may exceed the server's cap: {sent:?}"
    );
    assert_eq!(sent, vec![omnibus_shared::MAX_VALIDATOR_QUERY, 1]);
}

#[tokio::test]
async fn sweep_validators_applies_answers_from_every_chunk() {
    let first_uuid = "u-multi-chunk-first";
    let second_uuid = "u-multi-chunk-second";
    seed_complete_download(first_uuid, DlFormat::Epub, Some("\"old-first\""));
    seed_complete_download(second_uuid, DlFormat::Epub, Some("\"old-second\""));

    // `first_uuid` lands in the first chunk, `second_uuid` in the second —
    // one query per remaining slot in between keeps the boundary exact.
    let mut queries = vec![validator_query(first_uuid)];
    queries.extend(
        (1..omnibus_shared::MAX_VALIDATOR_QUERY).map(|i| validator_query(&format!("u-fill-{i}"))),
    );
    queries.push(validator_query(second_uuid));

    let app = axum::Router::new().route(
        "/api/downloads/validators",
        axum::routing::post(
            |axum::Json(req): axum::Json<omnibus_shared::DownloadValidatorRequest>| async move {
                let files = req
                    .files
                    .iter()
                    .map(|q| omnibus_shared::DownloadValidator {
                        book_uuid: q.book_uuid.clone(),
                        format: q.format,
                        file_id: q.file_id,
                        etag: match q.book_uuid.as_str() {
                            "u-multi-chunk-first" => Some("\"new-first\"".into()),
                            "u-multi-chunk-second" => Some("\"new-second\"".into()),
                            _ => None,
                        },
                    })
                    .collect();
                axum::Json(omnibus_shared::DownloadValidatorResponse { files })
            },
        ),
    );
    let base = spawn_router(app).await;

    let ok = sweep_validators(&base, &queries).await;

    assert!(ok);
    assert!(
        is_marked_stale(first_uuid, DlFormat::Epub),
        "the first chunk's answer must be applied"
    );
    assert!(
        is_marked_stale(second_uuid, DlFormat::Epub),
        "the second chunk's answer must be applied too"
    );
}

#[tokio::test]
async fn sweep_validators_returns_false_when_a_chunk_fails_but_keeps_earlier_answers() {
    let first_uuid = "u-chunk-fail-first";
    seed_complete_download(first_uuid, DlFormat::Epub, Some("\"old\""));

    // The first request (containing `first_uuid`) succeeds; the second
    // (the lone entry pushed past the cap) fails every time.
    let mut queries = vec![validator_query(first_uuid)];
    queries.extend(
        (1..omnibus_shared::MAX_VALIDATOR_QUERY)
            .map(|i| validator_query(&format!("u-fail-fill-{i}"))),
    );
    queries.push(validator_query("u-chunk-fail-second"));

    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = calls.clone();
    let app = axum::Router::new().route(
        "/api/downloads/validators",
        axum::routing::post(
            move |axum::Json(req): axum::Json<omnibus_shared::DownloadValidatorRequest>| {
                let counted = counted.clone();
                async move {
                    let call = counted.fetch_add(1, Ordering::SeqCst);
                    if call == 0 {
                        let files = req
                            .files
                            .iter()
                            .map(|q| omnibus_shared::DownloadValidator {
                                book_uuid: q.book_uuid.clone(),
                                format: q.format,
                                file_id: q.file_id,
                                etag: if q.book_uuid == "u-chunk-fail-first" {
                                    Some("\"new\"".into())
                                } else {
                                    None
                                },
                            })
                            .collect();
                        axum::Json(omnibus_shared::DownloadValidatorResponse { files })
                            .into_response()
                    } else {
                        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
                    }
                }
            },
        ),
    );
    let base = spawn_router(app).await;

    let ok = sweep_validators(&base, &queries).await;

    assert!(
        !ok,
        "a failed chunk must not report the sweep as successful"
    );
    assert!(
        is_marked_stale(first_uuid, DlFormat::Epub),
        "the chunk that succeeded before the failure must still be applied"
    );
}

#[tokio::test]
async fn refresh_stale_flags_leaves_the_timestamp_alone_when_the_sweep_fails() {
    // `last_stale_check` is a single process-wide static; serialize with the
    // other test that touches it so the two can't race each other's resets.
    let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
    last_stale_check().store(0, Ordering::Relaxed);
    seed_complete_download("u-refresh-fail", DlFormat::Epub, Some("\"old\""));

    let app = axum::Router::new().route(
        "/api/downloads/validators",
        axum::routing::post(|| async { (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "boom") }),
    );
    let base = spawn_router(app).await;

    refresh_stale_flags(&base).await;

    assert_eq!(
        last_stale_check().load(Ordering::Relaxed),
        0,
        "a failed sweep must not be recorded as done, so the next tick retries in full"
    );
}

#[tokio::test]
async fn refresh_stale_flags_advances_the_timestamp_after_a_successful_sweep() {
    let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
    last_stale_check().store(0, Ordering::Relaxed);
    seed_complete_download("u-refresh-ok", DlFormat::Epub, Some("\"old\""));

    let app = axum::Router::new().route(
        "/api/downloads/validators",
        axum::routing::post(
            |axum::Json(req): axum::Json<omnibus_shared::DownloadValidatorRequest>| async move {
                let files = req
                    .files
                    .into_iter()
                    .map(|q| omnibus_shared::DownloadValidator {
                        book_uuid: q.book_uuid,
                        format: q.format,
                        file_id: q.file_id,
                        etag: None,
                    })
                    .collect();
                axum::Json(omnibus_shared::DownloadValidatorResponse { files })
            },
        ),
    );
    let base = spawn_router(app).await;

    refresh_stale_flags(&base).await;

    assert!(
        last_stale_check().load(Ordering::Relaxed) > 0,
        "a fully successful sweep must advance the TTL timestamp"
    );
}
