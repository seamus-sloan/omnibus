//! ISBN metadata-lookup tests: the Open Library → Google Books provider chain
//! (against `wiremock`), the both-miss unresolved signal, and ISBN
//! normalization / validation.

use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::metadata_lookup::MetadataProvider;

use super::*;

const OL_PATH: &str = "/api/books";
const GB_PATH: &str = "/books/v1/volumes";
// Effective Java: valid ISBN-13, its ISBN-10 (0134685997), and a bad check digit.
const ISBN13: &str = "9780134685991";

fn config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: server.uri(),
        googlebooks_base: server.uri(),
        timeout: Duration::from_secs(5),
    }
}

async fn mount_ol(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(OL_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_gb(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

fn ol_hit() -> serde_json::Value {
    json!({
        format!("ISBN:{ISBN13}"): {
            "title": "Effective Java",
            "authors": [{ "name": "Joshua Bloch" }],
            "publish_date": "2018",
            "number_of_pages": 416,
            "publishers": [{ "name": "Addison-Wesley" }],
            "cover": { "large": "https://covers.openlibrary.org/b/id/1-L.jpg" }
        }
    })
}

fn gb_hit() -> serde_json::Value {
    json!({
        "totalItems": 1,
        "items": [{ "volumeInfo": {
            "title": "Effective Java",
            "authors": ["Joshua Bloch"],
            "publishedDate": "2018-01-01",
            "pageCount": 416,
            "publisher": "Addison-Wesley",
            "description": "The definitive guide.",
            "imageLinks": { "thumbnail": "http://books.google.com/x.jpg" }
        }}]
    })
}

// ── provider chain (AC1–AC3) ─────────────────────────────────────

#[tokio::test]
async fn lookup_resolves_via_open_library() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;

    let meta = lookup_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
    assert_eq!(meta.title, "Effective Java");
    assert_eq!(meta.authors, vec!["Joshua Bloch".to_string()]);
    assert_eq!(meta.pages, Some(416));
    assert_eq!(meta.isbn13, ISBN13);
    assert!(meta.cover_url.is_some());
}

#[tokio::test]
async fn lookup_falls_through_to_google_books_on_open_library_miss() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await; // empty body = ISBN unknown
    mount_gb(&server, gb_hit()).await;

    let meta = lookup_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
    assert_eq!(meta.title, "Effective Java");
    assert_eq!(meta.description.as_deref(), Some("The definitive guide."));
    // Google Books' `http://` cover link is upgraded to https so it isn't
    // blocked as mixed content on the scan result page.
    assert_eq!(
        meta.cover_url.as_deref(),
        Some("https://books.google.com/x.jpg")
    );
}

#[tokio::test]
async fn lookup_falls_through_to_google_books_on_open_library_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(OL_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    mount_gb(&server, gb_hit()).await;

    let meta = lookup_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn lookup_returns_unresolved_when_both_providers_miss() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;

    let result = lookup_isbn(&config_for(&server), ISBN13).await.unwrap();
    assert!(
        result.is_none(),
        "both-miss must be unresolved, not an error"
    );
}

#[tokio::test]
async fn lookup_surfaces_provider_error_when_fallback_fails() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = lookup_isbn(&config_for(&server), ISBN13).await.unwrap_err();
    assert!(matches!(err, MetadataLookupError::Provider(_)));
}

#[tokio::test]
async fn lookup_rejects_invalid_isbn_without_calling_a_provider() {
    // No mocks mounted: if validation didn't short-circuit, the request would
    // 404 against the mock server and this would be a Provider error instead.
    let server = MockServer::start().await;
    let err = lookup_isbn(&config_for(&server), "12345")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        MetadataLookupError::Isbn(IsbnError::InvalidLength(5))
    ));
}

// ── ISBN normalization (AC4) ─────────────────────────────────────

#[test]
fn normalize_passes_through_valid_isbn13() {
    assert_eq!(normalize_isbn(ISBN13).unwrap(), ISBN13);
}

#[test]
fn normalize_strips_hyphens_and_spaces() {
    assert_eq!(normalize_isbn("978-0-13-468599-1").unwrap(), ISBN13);
    assert_eq!(normalize_isbn("978 0 13 468599 1").unwrap(), ISBN13);
}

#[test]
fn normalize_converts_isbn10_to_isbn13() {
    assert_eq!(normalize_isbn("0134685997").unwrap(), ISBN13);
    assert_eq!(normalize_isbn("0-13-468599-7").unwrap(), ISBN13);
}

#[test]
fn normalize_accepts_isbn10_with_x_check_digit() {
    // 123456789X is a valid ISBN-10 (check digit X = 10) → ISBN-13 9781234567897.
    assert_eq!(normalize_isbn("123456789X").unwrap(), "9781234567897");
    assert_eq!(normalize_isbn("123456789x").unwrap(), "9781234567897");
}

#[test]
fn normalize_rejects_bad_isbn13_check_digit() {
    assert_eq!(
        normalize_isbn("9780134685990"),
        Err(IsbnError::InvalidCheckDigit)
    );
}

#[test]
fn normalize_rejects_bad_isbn10_check_digit() {
    assert_eq!(
        normalize_isbn("0134685996"),
        Err(IsbnError::InvalidCheckDigit)
    );
}

#[test]
fn normalize_rejects_wrong_length() {
    assert_eq!(normalize_isbn("12345"), Err(IsbnError::InvalidLength(5)));
    assert_eq!(normalize_isbn(""), Err(IsbnError::InvalidLength(0)));
}

#[test]
fn normalize_rejects_non_digit_characters() {
    // 13 chars but with letters where digits belong.
    assert_eq!(
        normalize_isbn("97801346859AB"),
        Err(IsbnError::InvalidChars)
    );
    // `X` is only legal as an ISBN-10 trailing check digit, not mid-number.
    assert_eq!(normalize_isbn("01X3468599"), Err(IsbnError::InvalidChars));
}

#[test]
fn normalize_rejects_non_ascii_digit_lookalikes() {
    // Fullwidth digits (U+FF10..) are Unicode "digits" but not ASCII — a valid
    // ISBN never contains them, so they're rejected rather than folded.
    assert_eq!(
        normalize_isbn("９７８０１３４６８５９９１"),
        Err(IsbnError::InvalidChars)
    );
}
