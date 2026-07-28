//! Unit tests for the content validator and the conditional requests built
//! on it. Drives [`serve_file`] directly rather than through the router —
//! none of this behaviour depends on auth or uuid resolution.

use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};

use super::*;

/// A scratch directory unique to this test, mirroring the pid + nanos
/// convention the REST fixtures use so concurrent runs never collide.
struct Scratch(std::path::PathBuf);

impl Scratch {
    fn new(tag: &str) -> Self {
        let pid = std::process::id();
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir = std::env::temp_dir().join(format!("omnibus_validator_{tag}_{pid}_{nanos}"));
        std::fs::create_dir_all(&dir).expect("create scratch dir");
        Self(dir)
    }

    fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = self.0.join(name);
        std::fs::write(&path, bytes).expect("write scratch file");
        path
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn get() -> axum::http::request::Builder {
    Request::builder().uri("/").method("GET")
}

async fn body_of(res: axum::response::Response) -> Vec<u8> {
    to_bytes(res.into_body(), usize::MAX)
        .await
        .expect("read body")
        .to_vec()
}

fn etag_of(res: &axum::response::Response) -> Option<String> {
    res.headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
}

#[tokio::test]
async fn file_etag_matches_the_validator_the_db_layer_reports_for_the_same_stat() {
    let scratch = Scratch::new("etag_agrees");
    let path = scratch.write("book.epub", b"0123456789");

    let meta = std::fs::metadata(&path).expect("stat");
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .expect("mtime");

    // The whole contract: a client comparing the `ETag` it downloaded under
    // against the validator in a metadata refresh must see the same string.
    assert_eq!(
        file_etag(&path).await,
        omnibus_shared::content_validator(mtime, 10)
    );
}

#[tokio::test]
async fn file_etag_is_none_for_a_path_that_cannot_be_stat_ed() {
    let scratch = Scratch::new("etag_missing");
    assert_eq!(file_etag(&scratch.0.join("absent.epub")).await, None);
}

#[test]
fn if_none_match_hits_matches_an_exact_tag_a_list_entry_and_a_wildcard() {
    let mut headers = HeaderMap::new();

    headers.insert(header::IF_NONE_MATCH, "\"abc-1\"".parse().unwrap());
    assert!(if_none_match_hits(&headers, "\"abc-1\""));
    assert!(!if_none_match_hits(&headers, "\"abc-2\""));

    // Browsers legitimately send the list form when they hold more than one
    // cached variant.
    headers.insert(
        header::IF_NONE_MATCH,
        "\"other\", \"abc-1\"".parse().unwrap(),
    );
    assert!(if_none_match_hits(&headers, "\"abc-1\""));

    headers.insert(header::IF_NONE_MATCH, "*".parse().unwrap());
    assert!(if_none_match_hits(&headers, "\"abc-1\""));
}

#[test]
fn if_none_match_hits_is_false_when_the_header_is_absent() {
    assert!(!if_none_match_hits(&HeaderMap::new(), "\"abc-1\""));
}

#[tokio::test]
async fn serve_file_returns_200_with_the_validator_stamped_on_it() {
    let scratch = Scratch::new("serve_200");
    let path = scratch.write("book.epub", b"0123456789");

    let res = serve_file(get().body(Body::empty()).unwrap(), &path).await;

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(etag_of(&res), file_etag(&path).await);
    assert_eq!(
        res.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some(REVALIDATE),
    );
    assert_eq!(
        res.headers()
            .get(header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some(MEDIA_VARY),
    );
    assert_eq!(body_of(res).await, b"0123456789");
}

#[tokio::test]
async fn serve_file_returns_a_bodyless_304_when_if_none_match_names_the_current_bytes() {
    let scratch = Scratch::new("serve_304");
    let path = scratch.write("book.epub", b"0123456789");
    let etag = file_etag(&path).await.expect("real file has a validator");

    let res = serve_file(
        get()
            .header(header::IF_NONE_MATCH, &etag)
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(etag_of(&res).as_deref(), Some(etag.as_str()));
    // A 304 that dropped these would reset the client's freshness bookkeeping
    // and reopen the cross-user cache gap `MEDIA_VARY` closes.
    assert_eq!(
        res.headers()
            .get(header::CACHE_CONTROL)
            .and_then(|v| v.to_str().ok()),
        Some(REVALIDATE),
    );
    assert_eq!(
        res.headers()
            .get(header::VARY)
            .and_then(|v| v.to_str().ok()),
        Some(MEDIA_VARY),
    );
    assert!(body_of(res).await.is_empty());
}

#[tokio::test]
async fn serve_file_returns_206_for_a_plain_range_request() {
    let scratch = Scratch::new("serve_206");
    let path = scratch.write("book.epub", b"0123456789");

    let res = serve_file(
        get()
            .header(header::RANGE, "bytes=5-")
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_of(res).await, b"56789");
}

#[tokio::test]
async fn serve_file_honours_a_range_whose_if_range_still_matches() {
    let scratch = Scratch::new("if_range_hit");
    let path = scratch.write("book.epub", b"0123456789");
    let etag = file_etag(&path).await.expect("real file has a validator");

    let res = serve_file(
        get()
            .header(header::RANGE, "bytes=5-")
            .header(header::IF_RANGE, &etag)
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    assert_eq!(res.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(body_of(res).await, b"56789");
}

#[tokio::test]
async fn serve_file_returns_the_whole_body_when_if_range_no_longer_matches() {
    // The splice: a download interrupted at byte 5 of the old file resumes
    // against a file that changed underneath it. Answering with a 206 counted
    // from the *new* bytes would leave the client appending a new tail to an
    // old head and calling the result a book. `ServeFile` alone would do
    // exactly that — it never looks at `If-Range`.
    let scratch = Scratch::new("if_range_stale");
    let path = scratch.write("book.epub", b"0123456789");
    let stale = file_etag(&path).await.expect("real file has a validator");

    // Replace in place with a file of a different length, so the validator
    // moves without depending on clock granularity.
    scratch.write("book.epub", b"ABCDEFGHIJKLMNO");
    assert_ne!(file_etag(&path).await.as_deref(), Some(stale.as_str()));

    let res = serve_file(
        get()
            .header(header::RANGE, "bytes=5-")
            .header(header::IF_RANGE, &stale)
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    assert_eq!(res.status(), StatusCode::OK, "must not be a 206 splice");
    assert_eq!(body_of(res).await, b"ABCDEFGHIJKLMNO");
}

#[tokio::test]
async fn serve_file_ignores_a_range_when_if_range_carries_a_date_it_cannot_verify() {
    // `If-Range` also admits an HTTP-date. Rather than compare clocks, the
    // conservative answer is the full body — wasteful at worst, never a
    // mismatched splice.
    let scratch = Scratch::new("if_range_date");
    let path = scratch.write("book.epub", b"0123456789");

    let res = serve_file(
        get()
            .header(header::RANGE, "bytes=5-")
            .header(header::IF_RANGE, "Wed, 21 Oct 2015 07:28:00 GMT")
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_of(res).await, b"0123456789");
}

#[tokio::test]
async fn serve_file_passes_a_404_through_without_a_validator() {
    let scratch = Scratch::new("serve_404");
    let path = scratch.0.join("absent.epub");

    let res = serve_file(get().body(Body::empty()).unwrap(), &path).await;

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
    assert_eq!(etag_of(&res), None);
}

#[tokio::test]
async fn serve_file_refuses_a_conditional_range_it_cannot_evaluate() {
    // No validator (the file vanished between plan and serve) means no way to
    // prove the client's range is still meaningful, so the `Range` goes.
    let scratch = Scratch::new("if_range_unstattable");
    let path = scratch.0.join("absent.epub");

    let req = enforce_if_range(
        get()
            .header(header::RANGE, "bytes=5-")
            .header(header::IF_RANGE, "\"whatever\"")
            .body(Body::empty())
            .unwrap(),
        file_etag(&path).await.as_deref(),
    );

    assert!(!req.headers().contains_key(header::RANGE));
}

#[tokio::test]
async fn enforce_if_range_leaves_a_range_alone_when_no_precondition_was_sent() {
    let scratch = Scratch::new("if_range_absent");
    let path = scratch.write("book.epub", b"0123456789");

    let req = enforce_if_range(
        get()
            .header(header::RANGE, "bytes=5-")
            .body(Body::empty())
            .unwrap(),
        file_etag(&path).await.as_deref(),
    );

    assert_eq!(
        req.headers()
            .get(header::RANGE)
            .and_then(|v| v.to_str().ok()),
        Some("bytes=5-"),
    );
}

#[tokio::test]
async fn with_source_validator_names_the_row_a_file_response_drew_from() {
    let scratch = Scratch::new("source_header");
    let path = scratch.write("book.epub", b"0123456789");
    let res = serve_file(get().body(Body::empty()).unwrap(), &path).await;

    let stamped = with_source_validator(res, Some("\"whole-row-9\""));

    // Deliberately not the `ETag`: on the audiobook and export routes the
    // bytes served are one part or a rewritten copy, while this names the row
    // a staleness check compares against.
    assert_eq!(
        stamped
            .headers()
            .get(SOURCE_VALIDATOR)
            .and_then(|v| v.to_str().ok()),
        Some("\"whole-row-9\""),
    );
    assert_ne!(
        stamped
            .headers()
            .get(header::ETAG)
            .and_then(|v| v.to_str().ok()),
        Some("\"whole-row-9\""),
    );
}

#[tokio::test]
async fn with_source_validator_leaves_a_bodyless_response_alone() {
    // A 304 carries no bytes, so there is no source for it to name.
    let scratch = Scratch::new("source_header_304");
    let path = scratch.write("book.epub", b"0123456789");
    let etag = file_etag(&path).await.expect("real file has a validator");
    let res = serve_file(
        get()
            .header(header::IF_NONE_MATCH, &etag)
            .body(Body::empty())
            .unwrap(),
        &path,
    )
    .await;

    let stamped = with_source_validator(res, Some("\"whole-row-9\""));

    assert_eq!(stamped.status(), StatusCode::NOT_MODIFIED);
    assert!(stamped.headers().get(SOURCE_VALIDATOR).is_none());
}
