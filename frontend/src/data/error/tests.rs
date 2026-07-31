//! [`DataError`] variant behavior and `server_error_message` rendering.

use super::*;

#[test]
fn unauthorized_reports_is_unauthorized() {
    assert!(DataError::Unauthorized.is_unauthorized());
    assert_eq!(DataError::Unauthorized.to_string(), "unauthorized");
}

#[test]
fn http_carries_status_and_is_not_unauthorized() {
    let err = DataError::Http {
        status: 400,
        body: "bad request".into(),
    };
    assert!(!err.is_unauthorized());
    // Display intentionally omits the body — callers that need it match
    // on the `Http { body, .. }` variant directly.
    assert_eq!(err.to_string(), "server returned 400");
}

#[test]
fn other_round_trips_its_message() {
    let err = DataError::Other("missing value field".into());
    assert!(!err.is_unauthorized());
    assert_eq!(err.to_string(), "missing value field");
}

#[test]
fn server_error_message_splices_the_body_back_in_for_http_errors() {
    let err = DataError::Http {
        status: 409,
        body: "book has no EPUB file to export next to".into(),
    };
    assert_eq!(
        server_error_message(&err),
        "409: book has no EPUB file to export next to"
    );
}

#[test]
fn server_error_message_falls_back_to_display_for_an_empty_body() {
    let err = DataError::Http {
        status: 500,
        body: String::new(),
    };
    assert_eq!(server_error_message(&err), "server returned 500");
}

#[test]
fn server_error_message_falls_back_to_display_for_non_http_variants() {
    let err = DataError::Other("book not found".into());
    assert_eq!(server_error_message(&err), "book not found");
}

// `Decode` only exists on the web/mobile builds that link `serde_json`
// for direct body deserialization (see the variant's doc comment); gated
// to match so this test simply doesn't compile into a server-only build.
#[cfg(any(feature = "mobile", feature = "web"))]
#[test]
fn decode_wraps_a_malformed_json_body_parse_failure() {
    // A truncated/invalid body run through the same `serde_json`
    // deserializer the web/SSR path uses, surfaced through `#[from]` as
    // `DataError::Decode`.
    let src = serde_json::from_str::<serde_json::Value>("{not valid json").unwrap_err();
    let err: DataError = src.into();
    assert!(matches!(err, DataError::Decode(_)), "got {err:?}");
    assert!(
        err.to_string()
            .starts_with("response deserialization failed"),
        "got {err}"
    );
}
