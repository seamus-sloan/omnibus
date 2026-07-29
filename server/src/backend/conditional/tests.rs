//! Tests for the media endpoints' conditional-request handling.
use std::time::{Duration, SystemTime};

use axum::http::{header, HeaderMap, HeaderName, StatusCode};

use super::*;

const ETAG: &str = "\"7b-1c8.00000000-1000\"";
const LEN: u64 = 1000;

/// A validator whose `Last-Modified` is a fixed, whole-second instant so the
/// date-form tests can name it exactly.
fn validator() -> Validator {
    Validator {
        etag: ETAG.to_string(),
        last_modified: SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000),
    }
}

fn headers(fields: &[(HeaderName, &str)]) -> HeaderMap {
    let mut map = HeaderMap::new();
    for (name, value) in fields {
        map.insert(name.clone(), value.parse().expect("header value"));
    }
    map
}

/// The `Range` `evaluate` resolved, or a panic when it answered otherwise.
fn range_of(outcome: Precondition) -> RangeOutcome {
    match outcome {
        Precondition::Proceed { range } => range,
        Precondition::NotModified(_) => panic!("expected the request to proceed"),
        Precondition::Failed => panic!("expected the request to proceed"),
    }
}

fn is_not_modified(outcome: &Precondition) -> bool {
    matches!(outcome, Precondition::NotModified(_))
}

fn is_failed(outcome: &Precondition) -> bool {
    matches!(outcome, Precondition::Failed)
}

fn http_date(secs: u64) -> String {
    httpdate::fmt_http_date(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
}

// --- If-None-Match (step 3) ---

#[test]
fn evaluate_answers_not_modified_when_if_none_match_carries_the_current_validator() {
    let outcome = evaluate(
        &headers(&[(header::IF_NONE_MATCH, ETAG)]),
        &validator(),
        LEN,
    );
    assert!(is_not_modified(&outcome));
}

#[test]
fn evaluate_carries_the_current_validator_back_on_a_304() {
    // The 304 must echo the *current* validator, not whatever variant the
    // client happened to send — a weak-prefixed request still gets the
    // canonical strong tag back.
    let weak = format!("W/{ETAG}");
    match evaluate(
        &headers(&[(header::IF_NONE_MATCH, &weak)]),
        &validator(),
        LEN,
    ) {
        Precondition::NotModified(etag) => assert_eq!(etag, ETAG),
        _ => panic!("expected a 304"),
    }
}

#[test]
fn evaluate_answers_not_modified_for_a_star_if_none_match() {
    let outcome = evaluate(&headers(&[(header::IF_NONE_MATCH, "*")]), &validator(), LEN);
    assert!(is_not_modified(&outcome));
}

#[test]
fn evaluate_answers_not_modified_when_any_if_none_match_list_entry_matches() {
    let list = format!("\"other\", {ETAG} , \"another\"");
    let outcome = evaluate(
        &headers(&[(header::IF_NONE_MATCH, &list)]),
        &validator(),
        LEN,
    );
    assert!(is_not_modified(&outcome));
}

#[test]
fn evaluate_proceeds_when_if_none_match_is_stale() {
    let outcome = evaluate(
        &headers(&[(header::IF_NONE_MATCH, "\"stale\"")]),
        &validator(),
        LEN,
    );
    assert!(!is_not_modified(&outcome));
}

// --- Precedence between the ETag and date conditions ---

#[test]
fn a_stale_if_none_match_wins_over_a_matching_if_modified_since() {
    // RFC 9110 §13.2.2: step 4 runs only when step 3's header is absent. The
    // client has just said its copy is out of date; answering 304 because a
    // date on the same request happens to match would hand it back the copy
    // it told us was wrong. This is exactly what leaving the date conditions
    // to a downstream file service would have produced.
    let outcome = evaluate(
        &headers(&[
            (header::IF_NONE_MATCH, "\"stale\""),
            (header::IF_MODIFIED_SINCE, &http_date(1_700_000_000)),
        ]),
        &validator(),
        LEN,
    );
    assert!(
        !is_not_modified(&outcome),
        "a non-matching If-None-Match owes the full body"
    );
}

#[test]
fn if_modified_since_alone_still_answers_not_modified() {
    let outcome = evaluate(
        &headers(&[(header::IF_MODIFIED_SINCE, &http_date(1_700_000_000))]),
        &validator(),
        LEN,
    );
    assert!(is_not_modified(&outcome));
}

#[test]
fn if_modified_since_older_than_the_file_serves_the_body() {
    let outcome = evaluate(
        &headers(&[(header::IF_MODIFIED_SINCE, &http_date(1_699_999_999))]),
        &validator(),
        LEN,
    );
    assert!(!is_not_modified(&outcome));
}

#[test]
fn an_unparseable_if_modified_since_is_ignored() {
    let outcome = evaluate(
        &headers(&[(header::IF_MODIFIED_SINCE, "not-a-date")]),
        &validator(),
        LEN,
    );
    assert!(!is_not_modified(&outcome));
}

// --- If-Match / If-Unmodified-Since (steps 1 and 2) ---

#[test]
fn a_non_matching_if_match_fails_the_precondition() {
    let outcome = evaluate(
        &headers(&[(header::IF_MATCH, "\"someone-elses\"")]),
        &validator(),
        LEN,
    );
    assert!(is_failed(&outcome), "If-Match mismatch is 412, not 404");
}

#[test]
fn a_matching_if_match_proceeds_and_a_star_always_does() {
    assert!(!is_failed(&evaluate(
        &headers(&[(header::IF_MATCH, ETAG)]),
        &validator(),
        LEN
    )));
    assert!(!is_failed(&evaluate(
        &headers(&[(header::IF_MATCH, "*")]),
        &validator(),
        LEN
    )));
}

#[test]
fn a_weak_if_match_fails_because_the_comparison_is_strong() {
    let weak = format!("W/{ETAG}");
    let outcome = evaluate(&headers(&[(header::IF_MATCH, &weak)]), &validator(), LEN);
    assert!(is_failed(&outcome));
}

#[test]
fn if_unmodified_since_fails_when_the_file_is_newer() {
    let outcome = evaluate(
        &headers(&[(header::IF_UNMODIFIED_SINCE, &http_date(1_699_999_999))]),
        &validator(),
        LEN,
    );
    assert!(is_failed(&outcome));
}

#[test]
fn if_unmodified_since_is_skipped_when_if_match_is_present() {
    // Step 2 runs only when step 1's header is absent, so a passing If-Match
    // must not be overruled by a date that would have failed on its own.
    let outcome = evaluate(
        &headers(&[
            (header::IF_MATCH, ETAG),
            (header::IF_UNMODIFIED_SINCE, &http_date(1_699_999_999)),
        ]),
        &validator(),
        LEN,
    );
    assert!(!is_failed(&outcome));
}

// --- If-Range (step 5) ---

#[test]
fn evaluate_keeps_the_range_when_if_range_matches() {
    let range = range_of(evaluate(
        &headers(&[(header::IF_RANGE, ETAG), (header::RANGE, "bytes=100-")]),
        &validator(),
        LEN,
    ));
    assert_eq!(
        range,
        RangeOutcome::Partial {
            start: 100,
            end: 999
        },
        "a validated resume must still be served as a 206"
    );
}

#[test]
fn evaluate_drops_the_range_when_if_range_is_stale() {
    // The splice this module exists to prevent: without the drop, the
    // response would be a 206 whose offsets belong to the previous file.
    let range = range_of(evaluate(
        &headers(&[
            (header::IF_RANGE, "\"previous-file\""),
            (header::RANGE, "bytes=100-"),
        ]),
        &validator(),
        LEN,
    ));
    assert_eq!(range, RangeOutcome::Full);
}

#[test]
fn evaluate_drops_the_range_when_if_range_is_weak() {
    // RFC 9110 §13.1.5 requires strong comparison, so a weak tag can never
    // authorise a resume even when its opaque part matches.
    let weak = format!("W/{ETAG}");
    let range = range_of(evaluate(
        &headers(&[(header::IF_RANGE, &weak), (header::RANGE, "bytes=100-")]),
        &validator(),
        LEN,
    ));
    assert_eq!(range, RangeOutcome::Full);
}

#[test]
fn evaluate_honours_a_date_form_if_range_that_matches_last_modified() {
    let range = range_of(evaluate(
        &headers(&[
            (header::IF_RANGE, &http_date(1_700_000_000)),
            (header::RANGE, "bytes=100-"),
        ]),
        &validator(),
        LEN,
    ));
    assert_eq!(
        range,
        RangeOutcome::Partial {
            start: 100,
            end: 999
        }
    );
}

#[test]
fn evaluate_drops_the_range_when_a_date_form_if_range_disagrees() {
    let range = range_of(evaluate(
        &headers(&[
            (header::IF_RANGE, &http_date(1_699_999_999)),
            (header::RANGE, "bytes=100-"),
        ]),
        &validator(),
        LEN,
    ));
    assert_eq!(range, RangeOutcome::Full);
}

#[test]
fn evaluate_leaves_an_unconditional_range_alone() {
    // A bare Range asks for a slice, not a consistency guarantee — seeking
    // an audiobook must keep working.
    let range = range_of(evaluate(
        &headers(&[(header::RANGE, "bytes=0-1023")]),
        &validator(),
        LEN,
    ));
    assert_eq!(range, RangeOutcome::Partial { start: 0, end: 999 });
}

#[test]
fn evaluate_reports_an_unsatisfiable_range() {
    let range = range_of(evaluate(
        &headers(&[(header::RANGE, "bytes=5000-")]),
        &validator(),
        LEN,
    ));
    assert_eq!(range, RangeOutcome::NotSatisfiable);
}

// --- not_modified ---

#[test]
fn not_modified_carries_the_same_cache_policy_a_200_would() {
    let resp = not_modified(ETAG, MEDIA_CACHE_CONTROL, MEDIA_VARY);
    assert_eq!(resp.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(resp.headers().get(header::ETAG).unwrap(), ETAG);
    assert_eq!(
        resp.headers().get(header::CACHE_CONTROL).unwrap(),
        MEDIA_CACHE_CONTROL
    );
    assert_eq!(resp.headers().get(header::VARY).unwrap(), MEDIA_VARY);
}

// --- open: the validator and the bytes come from one handle ---

#[tokio::test]
async fn open_derives_a_validator_that_moves_when_the_file_is_replaced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("book.epub");
    tokio::fs::write(&path, b"first").await.expect("write");
    let before = open(&path).await.expect("open").validator.etag;

    // Replace via write-new-then-rename — how rsync, Calibre and most
    // editors put a corrected file back. Same length, and the copy carries
    // the original timestamp forward, so only the inode term distinguishes
    // them.
    let original_mtime = std::fs::metadata(&path).expect("stat").modified().unwrap();
    let replacement = dir.path().join("replacement");
    let file = std::fs::File::create(&replacement).expect("create");
    std::io::Write::write_all(&mut &file, b"secnd").expect("write");
    file.set_modified(original_mtime).expect("preserve mtime");
    drop(file);
    tokio::fs::rename(&replacement, &path)
        .await
        .expect("rename");

    let after = open(&path).await.expect("open").validator.etag;
    assert_ne!(
        before, after,
        "an mtime-preserving, same-length replacement must still move the validator"
    );
}

#[tokio::test]
async fn an_open_handle_keeps_serving_the_bytes_its_validator_describes() {
    // The TOCTOU this design closes: if the validator came from a path and
    // the bytes from a later re-open of that path, an atomic replace between
    // them would serve new bytes under an old validator — a client whose
    // `If-Range` matched would splice. Holding the handle makes that
    // impossible on Unix, where a rename cannot move an open inode.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("book.epub");
    tokio::fs::write(&path, b"original-contents")
        .await
        .expect("write");
    let opened = open(&path).await.expect("open");

    let replacement = dir.path().join("replacement");
    tokio::fs::write(&replacement, b"replacement-body")
        .await
        .expect("write");
    tokio::fs::rename(&replacement, &path)
        .await
        .expect("rename");

    let resp = serve(
        opened,
        RangeOutcome::Full,
        FileResponse {
            content_type: "application/epub+zip",
            cache_control: MEDIA_CACHE_CONTROL,
            disposition: None,
        },
    )
    .await;
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(
        body.as_ref(),
        b"original-contents",
        "the handle must keep serving the representation it was validated against"
    );
}

#[tokio::test]
async fn open_reports_a_missing_file_as_an_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(open(&dir.path().join("absent")).await.is_err());
}

// --- serve ---

#[tokio::test]
async fn serve_answers_a_partial_range_with_the_right_slice_and_content_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("book.epub");
    tokio::fs::write(&path, b"0123456789").await.expect("write");
    let opened = open(&path).await.expect("open");

    let resp = serve(
        opened,
        RangeOutcome::Partial { start: 2, end: 5 },
        FileResponse {
            content_type: "application/epub+zip",
            cache_control: MEDIA_CACHE_CONTROL,
            disposition: None,
        },
    )
    .await;
    assert_eq!(resp.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes 2-5/10"
    );
    assert_eq!(resp.headers().get(header::CONTENT_LENGTH).unwrap(), "4");
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .expect("body");
    assert_eq!(body.as_ref(), b"2345");
}

#[tokio::test]
async fn serve_answers_an_unsatisfiable_range_with_416_and_the_full_length() {
    // Reporting this as 404 would tell a resuming client the book had
    // disappeared, and discard the length it needs to restart correctly.
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("book.epub");
    tokio::fs::write(&path, b"0123456789").await.expect("write");
    let opened = open(&path).await.expect("open");

    let resp = serve(
        opened,
        RangeOutcome::NotSatisfiable,
        FileResponse {
            content_type: "application/epub+zip",
            cache_control: MEDIA_CACHE_CONTROL,
            disposition: None,
        },
    )
    .await;
    assert_eq!(resp.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(
        resp.headers().get(header::CONTENT_RANGE).unwrap(),
        "bytes */10"
    );
    assert!(resp.headers().get(header::ETAG).is_some());
}

#[tokio::test]
async fn serve_publishes_the_headers_a_conditional_client_needs_next_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("book.epub");
    tokio::fs::write(&path, b"0123456789").await.expect("write");
    let opened = open(&path).await.expect("open");
    let expected_etag = opened.validator.etag.clone();

    let resp = serve(
        opened,
        RangeOutcome::Full,
        FileResponse {
            content_type: "application/epub+zip",
            cache_control: MEDIA_CACHE_CONTROL,
            disposition: Some("attachment; filename=\"b.epub\"".into()),
        },
    )
    .await;
    assert_eq!(resp.status(), StatusCode::OK);
    let h = resp.headers();
    assert_eq!(h.get(header::ETAG).unwrap(), &expected_etag);
    // `Last-Modified` has to be published because `If-Modified-Since` is
    // honoured — a client can only condition on a value it was given.
    assert!(h.get(header::LAST_MODIFIED).is_some());
    assert_eq!(h.get(header::ACCEPT_RANGES).unwrap(), "bytes");
    assert_eq!(h.get(header::VARY).unwrap(), MEDIA_VARY);
    assert_eq!(h.get(header::CONTENT_TYPE).unwrap(), "application/epub+zip");
    assert_eq!(h.get(header::X_CONTENT_TYPE_OPTIONS).unwrap(), "nosniff");
    assert!(h
        .get(header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap()
        .starts_with("attachment"));
}
