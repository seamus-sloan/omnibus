//! ISBN metadata-lookup tests: the key-dependent provider chain (Open Library
//! first when keyless, Google Books first when a key is configured), run
//! against `wiremock`, plus the both-miss unresolved signal. Normalization
//! itself is tested in `omnibus_shared::isbn`, which owns it.

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
        // Keyless on purpose: the mock never checks it, and reading the real
        // env here would make the suite depend on the developer's `.env`.
        googlebooks_api_key: None,
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
async fn lookup_prefers_google_books_when_a_key_is_configured() {
    // Both providers would answer; the key makes Google Books the primary.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_gb(&server, gb_hit()).await;

    let config = MetadataLookupConfig {
        googlebooks_api_key: Some("k".into()),
        ..config_for(&server)
    };
    let meta = lookup_isbn(&config, ISBN13).await.unwrap().unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
}

#[tokio::test]
async fn lookup_prefers_open_library_when_no_key_is_configured() {
    // The keyless default: Open Library leads even when Google Books would hit.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_gb(&server, gb_hit()).await;

    let meta = lookup_isbn(&config_for(&server), ISBN13)
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
        googlebooks_api_key: Some("k".into()),
        ..config_for(&server)
    };
    let meta = lookup_isbn(&config, ISBN13).await.unwrap().unwrap();
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
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
    let err = lookup_isbn(&config_for(&server), "12345")
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        MetadataLookupError::Isbn(IsbnError::InvalidLength(5))
    ));
}

// ── invalid response bodies ──────────────────────────────────────

#[tokio::test]
async fn openlibrary_lookup_errors_when_response_body_is_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(OL_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = super::providers::openlibrary_lookup(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not valid json"),
        "must report a json parse failure, got: {err}"
    );
}

#[tokio::test]
async fn googlebooks_lookup_errors_when_response_body_is_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = super::providers::googlebooks_lookup(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not valid json"),
        "must report a json parse failure, got: {err}"
    );
}

// ── Google Books API key ─────────────────────────────────────────

#[test]
fn live_with_key_uses_live_endpoints_and_carries_the_supplied_key() {
    let config = MetadataLookupConfig::live_with_key(Some("resolved-key".into()));
    assert_eq!(config.openlibrary_base, "https://openlibrary.org");
    assert_eq!(config.googlebooks_base, "https://www.googleapis.com");
    assert_eq!(config.googlebooks_api_key.as_deref(), Some("resolved-key"));
    // None means a keyless (shared-quota) lookup.
    assert_eq!(
        MetadataLookupConfig::live_with_key(None).googlebooks_api_key,
        None
    );
}

#[test]
fn googlebooks_url_appends_the_key_only_when_configured() {
    let server_base = "http://gb.test";
    let keyless = MetadataLookupConfig {
        openlibrary_base: server_base.into(),
        googlebooks_base: server_base.into(),
        googlebooks_api_key: None,
        timeout: Duration::from_secs(5),
    };
    assert_eq!(
        super::providers::googlebooks_url(&keyless, ISBN13),
        format!("{server_base}/books/v1/volumes?q=isbn:{ISBN13}")
    );

    let keyed = MetadataLookupConfig {
        googlebooks_api_key: Some("sekret".into()),
        ..keyless
    };
    assert_eq!(
        super::providers::googlebooks_url(&keyed, ISBN13),
        format!("{server_base}/books/v1/volumes?q=isbn:{ISBN13}&key=sekret")
    );
}

#[tokio::test]
async fn googlebooks_failure_never_renders_the_api_key() {
    // The key rides in the query string, and a `reqwest::Error` renders the
    // request URL in its Display — so an un-stripped error would write the key
    // into the on-disk JSON log on every 429. Call the provider directly: under
    // the key-dependent ladder a keyed Google Books is the *primary*, so its
    // error is swallowed to a warn and a full `lookup_isbn` would fall through
    // to Open Library instead of returning it. This error object is what both
    // the warn log and any terminal Provider error render, so it's the leak
    // surface to guard.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let config = MetadataLookupConfig {
        googlebooks_api_key: Some("super-secret-key".into()),
        ..config_for(&server)
    };
    let err = super::providers::googlebooks_lookup(&config, ISBN13)
        .await
        .unwrap_err();
    // Walk the whole source chain — that's what `{:#}`/`{:?}` logging renders.
    let rendered = format!("{err:?} {err:#}");
    assert!(
        !rendered.contains("super-secret-key"),
        "api key leaked into the error chain: {rendered}"
    );
}

#[tokio::test]
async fn googlebooks_retries_a_transient_503_then_succeeds() {
    // Google Books intermittently answers 503 `backendFailed` for valid ISBNs.
    // Without a retry a single blip fails the whole scan.
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(503))
        .up_to_n_times(1)
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
async fn googlebooks_gives_up_after_the_retry_budget() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = lookup_isbn(&config_for(&server), ISBN13).await.unwrap_err();
    assert!(matches!(err, MetadataLookupError::Provider(_)));
    // The give-up message names the status but never the URL (which carries
    // the key); the reader still sees the outage sentence.
    let chain = format!("{err:?}");
    assert!(
        chain.contains("503"),
        "give-up must record the status: {chain}"
    );
    assert!(
        !chain.contains("key="),
        "url must not reach the log: {chain}"
    );
}

#[test]
fn publication_year_trims_google_dates_to_a_bare_year() {
    use super::providers::publication_year;
    // Google returns whatever precision it holds; Open Library gives a year.
    assert_eq!(publication_year("2025-02-25").as_deref(), Some("2025"));
    assert_eq!(publication_year("2025-02").as_deref(), Some("2025"));
    assert_eq!(publication_year("2025").as_deref(), Some("2025"));
    // Non-numeric precision is passed through, not guessed at.
    assert_eq!(publication_year("MMXXV").as_deref(), Some("MMXXV"));
    assert_eq!(publication_year("  "), None);
}

#[tokio::test]
async fn googlebooks_drops_a_zero_page_count_and_trims_the_year() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(
        &server,
        json!({
            "totalItems": 1,
            "items": [{ "volumeInfo": {
                "title": "Swordheart",
                "authors": ["T. Kingfisher"],
                "publishedDate": "2025-02-25",
                "pageCount": 0,
                "publisher": "Bramble"
            }}]
        }),
    )
    .await;

    let meta = lookup_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.year.as_deref(), Some("2025"));
    assert_eq!(meta.pages, None, "0 pages means unknown, not zero-length");
}
