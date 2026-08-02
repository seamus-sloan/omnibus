//! Shared fixtures for the metadata-lookup suite, plus the ladder's own tests:
//! which rung answers, in what order, and when a failure is allowed to
//! surface. Per-provider behaviour lives in the sibling modules, mirroring
//! `providers/`. Normalization itself is tested in `omnibus_shared::isbn`,
//! which owns it.

mod googlebooks_provider;
mod hardcover_provider;
mod openlibrary_provider;

use std::time::Duration;

use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::metadata_lookup::MetadataProvider;

use super::providers::{googlebooks, openlibrary};
use super::*;

const OL_PATH: &str = "/api/books";

const GB_PATH: &str = "/books/v1/volumes";

// Effective Java: valid ISBN-13, its ISBN-10 (0134685997), and a bad check digit.
const ISBN13: &str = "9780134685991";

fn config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: server.uri(),
        googlebooks_base: server.uri(),
        // Keyless on purpose: the mock never checks it, and reading the real
        // env here would make the suite depend on the developer's `.env`.
        hardcover_base: server.uri(),
        keys: ProviderKeys::default(),
        timeout: Duration::from_secs(5),
    }
}

/// Same as [`config_for`] but with a key configured — the bare-text fallback
/// gate requires one (#1614: keyless requests share Google's anonymous
/// quota, so the fallback only fires when a key is present).
fn keyed_config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("k".into()),
            ..ProviderKeys::default()
        },
        ..config_for(server)
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

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
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

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
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

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn lookup_prefers_google_books_when_a_key_is_configured() {
    // Both providers would answer; the key makes Google Books the primary.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_gb(&server, gb_hit()).await;

    let config = MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("k".into()),
            ..ProviderKeys::default()
        },
        ..config_for(&server)
    };
    let meta = search_provider_by_isbn(&config, ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn lookup_prefers_open_library_when_no_key_is_configured() {
    // The keyless default: Open Library leads even when Google Books would hit.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_gb(&server, gb_hit()).await;

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
}

#[tokio::test]
async fn lookup_with_key_falls_back_to_open_library_when_google_books_misses() {
    // Keyed Google Books is primary; on its miss the ladder still reaches
    // Open Library rather than giving up.
    let server = MockServer::start().await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;
    mount_ol(&server, ol_hit()).await;

    let config = MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("k".into()),
            ..ProviderKeys::default()
        },
        ..config_for(&server)
    };
    let meta = search_provider_by_isbn(&config, ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
}

#[tokio::test]
async fn lookup_returns_unresolved_when_both_providers_miss() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;

    let result = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap();
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

    let err = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
    assert!(matches!(err, MetadataLookupError::Provider(_)));
    // The rendered message is what reaches the reader, so it must describe the
    // outage rather than repeat the provider's own wording ("google books
    // returned an error status"), which reads as an Omnibus bug.
    let msg = err.to_string();
    assert!(
        msg.contains("temporarily unavailable"),
        "provider outage must read as an outage, got: {msg}"
    );
    assert!(
        !msg.contains("google"),
        "provider internals must not leak to the reader, got: {msg}"
    );
}

#[tokio::test]
async fn lookup_rejects_invalid_isbn_without_calling_a_provider() {
    // No mocks mounted: if validation didn't short-circuit, the request would
    // 404 against the mock server and this would be a Provider error instead.
    let server = MockServer::start().await;
    let err = search_provider_by_isbn(&config_for(&server), "12345")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        MetadataLookupError::Isbn(IsbnError::InvalidLength(5))
    ));
}

// ── Google Books API key ─────────────────────────────────────────

#[test]
fn live_uses_live_endpoints_and_carries_the_supplied_keys() {
    let config = MetadataLookupConfig::live(ProviderKeys {
        googlebooks: Some("resolved-key".into()),
        hardcover: Some("hc-key".into()),
    });
    assert_eq!(config.openlibrary_base, "https://openlibrary.org");
    assert_eq!(config.googlebooks_base, "https://www.googleapis.com");
    assert_eq!(
        config.hardcover_base,
        "https://api.hardcover.app/v1/graphql"
    );
    assert_eq!(config.keys.googlebooks.as_deref(), Some("resolved-key"));
    assert_eq!(config.keys.hardcover.as_deref(), Some("hc-key"));
    // Absent keys mean a keyless (shared-quota) Google Books and no Hardcover
    // rung at all.
    let keyless = MetadataLookupConfig::live(ProviderKeys::default());
    assert_eq!(keyless.keys.googlebooks, None);
    assert_eq!(keyless.keys.hardcover, None);
}

/// A config with no mock server behind it, for URL-builder assertions.
fn offline_config(key: Option<&str>) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: "http://ol.test".into(),
        googlebooks_base: "http://gb.test".into(),
        hardcover_base: "http://hc.test".into(),
        keys: ProviderKeys {
            googlebooks: key.map(str::to_string),
            hardcover: None,
        },
        timeout: Duration::from_secs(5),
    }
}

// ── title search ─────────────────────────────────────────────────

const OL_SEARCH_PATH: &str = "/search.json";

const QUERY: &str = "effective java";

fn ol_search_hit() -> serde_json::Value {
    json!({
        "docs": [
            {
                "title": "Effective Java",
                "author_name": ["Joshua Bloch"],
                "first_publish_year": 2001,
                "isbn": ["not-an-isbn", "0134685997", "9780134685991"],
                "cover_i": 8511809,
                "number_of_pages_median": 416,
            },
            // No usable ISBN in any edition: not actionable, must be skipped.
            {
                "title": "Effective Java Notes",
                "author_name": ["Someone Else"],
                "isbn": ["garbage"],
            },
        ]
    })
}

fn gb_search_hit() -> serde_json::Value {
    json!({
        "totalItems": 2,
        "items": [
            { "volumeInfo": {
                "title": "Effective Java",
                "authors": ["Joshua Bloch"],
                "publishedDate": "2018-01-01",
                "industryIdentifiers": [
                    { "type": "ISBN_10", "identifier": "0134685997" },
                    { "type": "ISBN_13", "identifier": "9780134685991" },
                ],
            }},
            // The same ISBN again (Google Books answers repeat editions):
            // deduped away by `search_title`.
            { "volumeInfo": {
                "title": "Effective Java (reprint)",
                "authors": ["Joshua Bloch"],
                "industryIdentifiers": [
                    { "type": "ISBN_13", "identifier": "9780134685991" },
                ],
            }},
        ]
    })
}

async fn mount_ol_search(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .and(query_param("title", QUERY))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_gb_search(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", QUERY))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

#[tokio::test]
async fn search_title_maps_open_library_docs_and_skips_isbnless_ones() {
    let server = MockServer::start().await;
    mount_ol_search(&server, ol_search_hit()).await;

    let results = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "the isbn-less doc must be skipped");
    let meta = &results[0];
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
    assert_eq!(meta.title, "Effective Java");
    assert_eq!(meta.authors, vec!["Joshua Bloch".to_string()]);
    // The first normalizable entry wins; the ISBN-10 folds to the same 13.
    assert_eq!(meta.isbn13, ISBN13);
    assert_eq!(meta.first_publish_year, Some(2001));
    assert_eq!(meta.pages, Some(416));
    assert_eq!(
        meta.cover_url.as_deref(),
        Some("https://covers.openlibrary.org/b/id/8511809-L.jpg")
    );
}

#[tokio::test]
async fn search_title_falls_through_to_google_books_when_open_library_is_empty() {
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, gb_search_hit()).await;

    let results = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "repeat editions must be deduped by isbn");
    assert_eq!(results[0].source, MetadataProvider::GoogleBooks);
    assert_eq!(results[0].isbn13, ISBN13);
    assert_eq!(results[0].year.as_deref(), Some("2018"));
}

#[tokio::test]
async fn search_title_falls_through_to_google_books_on_open_library_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    mount_gb_search(&server, gb_search_hit()).await;

    let results = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results[0].source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn search_title_prefers_google_books_when_a_key_is_configured() {
    let server = MockServer::start().await;
    mount_ol_search(&server, ol_search_hit()).await;
    mount_gb_search(&server, gb_search_hit()).await;

    let results = search_provider_by_title(&keyed_config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results[0].source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn search_title_returns_empty_when_both_providers_are_empty() {
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, json!({ "totalItems": 0 })).await;

    let results = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    assert!(
        results.is_empty(),
        "a double miss is an empty list, not an error"
    );
}

#[tokio::test]
async fn search_title_surfaces_provider_error_when_fallback_fails() {
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap_err();
    assert!(matches!(err, MetadataLookupError::Provider(_)));
}

#[tokio::test]
async fn search_title_caps_the_candidate_list() {
    let docs: Vec<serde_json::Value> = (0..20)
        .map(|i| {
            json!({
                "title": format!("Book {i}"),
                // Distinct valid ISBN-13s: vary the payload, recompute the check digit.
                "isbn": [with_check_digit(&format!("9780000000{i:02}"))],
            })
        })
        .collect();
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": docs })).await;

    let results = search_provider_by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), SEARCH_LIMIT);
}

/// Recompute the EAN-13 check digit for a 12-digit prefix.
fn with_check_digit(prefix12: &str) -> String {
    let sum: u32 = prefix12
        .chars()
        .enumerate()
        .map(|(i, c)| c.to_digit(10).unwrap() * if i % 2 == 0 { 1 } else { 3 })
        .sum();
    format!("{prefix12}{}", (10 - (sum % 10)) % 10)
}

#[test]
fn search_urls_percent_encode_the_query_and_carry_the_key() {
    let keyed = offline_config(Some("sekret"));
    let gb = googlebooks::search_url(&keyed, "war & peace").unwrap();
    assert!(
        gb.contains("q=war+%26+peace") && gb.ends_with("&key=sekret"),
        "got: {gb}"
    );
    let ol = openlibrary::search_url(&keyed, "war & peace").unwrap();
    assert!(
        ol.starts_with("http://ol.test/search.json?title=war+%26+peace"),
        "got: {ol}"
    );
}
