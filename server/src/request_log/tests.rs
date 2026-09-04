//! Tests for the request trace layer: the pure helpers, the Kobo path
//! redaction, and one end-to-end capture that drives a Range-served media
//! request through `rest_router` + [`super::layer`] under a scoped JSON
//! subscriber and reads the emitted records back the way the on-disk sink
//! would write them.

use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    body::{to_bytes, Body},
    http::{header, HeaderMap, HeaderValue, Request, StatusCode},
};
use omnibus_db::test_support::EnvVarGuard;
use tower::ServiceExt;
use tracing_subscriber::{fmt::MakeWriter, prelude::*};

use super::*;
use crate::{
    auth::test_support as auth_test_support,
    backend::{rest_router, test_support::*, AppState},
};

#[test]
fn header_str_returns_the_value_when_the_header_is_present() {
    let mut headers = HeaderMap::new();
    headers.insert(header::RANGE, HeaderValue::from_static("bytes=0-1"));
    assert_eq!(header_str(&headers, header::RANGE), "bytes=0-1");
}

#[test]
fn header_str_returns_empty_when_the_header_is_absent_or_not_utf8() {
    let mut headers = HeaderMap::new();
    assert_eq!(header_str(&headers, header::RANGE), "");
    headers.insert(header::RANGE, HeaderValue::from_bytes(b"\xff").unwrap());
    assert_eq!(header_str(&headers, header::RANGE), "");
}

#[test]
fn duration_ms_truncates_to_whole_milliseconds() {
    assert_eq!(duration_ms(Duration::from_micros(1_999)), 1);
    assert_eq!(duration_ms(Duration::from_secs(7_200)), 7_200_000);
}

#[test]
fn latency_ms_matches_the_tower_http_default_display() {
    assert_eq!(latency_ms(Duration::from_millis(1234)), "1234 ms");
    assert_eq!(latency_ms(Duration::from_micros(250)), "0 ms");
}

#[test]
fn redact_path_replaces_the_token_segment_of_a_kobo_path() {
    assert_eq!(
        redact_path("/kobo/abc123/v1/library/sync"),
        "/kobo/[REDACTED]/v1/library/sync"
    );
    assert_eq!(
        redact_path("/kobo/abc123/v1/download/some-uuid"),
        "/kobo/[REDACTED]/v1/download/some-uuid"
    );
}

#[test]
fn redact_path_leaves_non_kobo_paths_unchanged() {
    assert_eq!(redact_path("/api/ebooks"), "/api/ebooks");
    assert_eq!(redact_path("/"), "/");
    assert_eq!(redact_path("/kobo"), "/kobo");
}

/// Shared in-memory sink for a scoped JSON `fmt` layer — the same encoder
/// `logging::init_tracing` uses for the on-disk file, so what these tests
/// parse is what the admin log viewer would.
#[derive(Clone, Default)]
struct Sink(Arc<Mutex<Vec<u8>>>);

impl Sink {
    fn lines(&self) -> Vec<serde_json::Value> {
        let bytes = self.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    }

    fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}

impl io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> MakeWriter<'a> for Sink {
    type Writer = Sink;

    fn make_writer(&'a self) -> Sink {
        self.clone()
    }
}

fn record_with_message<'a>(lines: &'a [serde_json::Value], message: &str) -> &'a serde_json::Value {
    lines
        .iter()
        .find(|l| l["fields"]["message"] == message)
        .unwrap_or_else(|| panic!("no record with message {message:?} in {lines:#?}"))
}

#[tokio::test]
async fn layer_logs_client_ip_range_and_bytes_served_without_the_query_token() {
    // Rule 03 "no ambient environment": a developer `.env` may set the
    // forwarded-for opt-in, and AC5 is specifically about it being unset.
    let _env = EnvVarGuard::set("OMNIBUS_TRUST_FORWARDED_FOR", None);

    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let library_path = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join("Author/Book")).unwrap();
    let payload: Vec<u8> = (0u8..100).collect();
    std::fs::File::create(dir.path().join("Author/Book/01.mp3"))
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
        &[(0, "Author/Book/01.mp3", 60.0)],
    )
    .await;

    let app = rest_router(AppState::new(pool)).layer(layer());
    let req = Request::builder()
        .uri(format!("/api/audiobooks/{uuid}/parts/0?token={token}"))
        .header(header::RANGE, "bytes=0-1")
        .header("x-forwarded-for", "203.0.113.9")
        .header(header::USER_AGENT, "omnibus-test/1")
        .body(Body::empty())
        .unwrap();

    let sink = Sink::default();
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .json()
            .with_writer(sink.clone()),
    );
    // Thread-scoped, not global: `#[tokio::test]` is current-thread, so the
    // whole oneshot — including the body poll that fires `on_eos` — runs
    // under it, and other tests in the binary are unaffected.
    let guard = tracing::subscriber::set_default(subscriber);
    let res = app.oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    // `on_eos` fires when the body is polled to its end, not when the
    // headers return — so drain it before reading the sink.
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body.as_ref(), &payload[..2]);
    drop(guard);

    let lines = sink.lines();
    let done = record_with_message(&lines, "finished processing request");
    let span = &done["span"];
    assert_eq!(span["name"], "request");
    assert_eq!(span["method"], "GET");
    assert_eq!(span["path"], format!("/api/audiobooks/{uuid}/parts/0"));
    assert_eq!(span["range"], "bytes=0-1");
    assert_eq!(span["user_agent"], "omnibus-test/1");
    // AC5: no ConnectInfo on a oneshot and the opt-in unset, so the resolver
    // falls back to the unspecified address — never the forwarded header.
    assert_eq!(span["client_ip"], "0.0.0.0");
    let fields = &done["fields"];
    assert_eq!(fields["status"], 206);
    assert_eq!(fields["content_length"], "2");
    assert_eq!(fields["content_range"], "bytes 0-1/100");
    assert!(
        fields["latency"].as_str().unwrap().ends_with(" ms"),
        "latency keeps tower-http's `{{n}} ms` shape: {fields:#?}"
    );

    let eos = record_with_message(&lines, "response body finished");
    assert!(eos["fields"]["stream_duration_ms"].is_u64());
    assert_eq!(eos["span"]["name"], "request");
    assert_eq!(eos["span"]["path"], span["path"]);

    // AC3: the media token rides the query string, which no field records.
    let text = sink.text();
    assert!(!text.contains(&token), "token leaked into the log: {text}");
    assert!(!text.contains("token="), "query string leaked: {text}");
    assert!(!text.contains("203.0.113.9"), "forwarded IP leaked: {text}");
}
