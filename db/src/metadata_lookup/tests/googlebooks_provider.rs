//! Google Books provider tests: the API-key handling that must never leak, the
//! retry budget, and the bare-text fallback for a field search that answers
//! empty. Ladder-level ordering lives in the parent module.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers;
use super::super::providers::{googlebooks, publication_year};
use super::super::*;
use super::{
    config_for, gb_hit, keyed_config_for, mount_gb, mount_ol, offline_config, title_query,
    title_query_isbn, GB_PATH, ISBN10, ISBN13,
};

#[tokio::test]
async fn googlebooks_lookup_errors_when_response_body_is_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not valid json"),
        "must report a json parse failure, got: {err}"
    );
}

#[test]
fn googlebooks_urls_are_the_same_shape_with_or_without_a_key() {
    // The key moved to a header (#2131), so a configured one changes nothing
    // about the URL — that identity is the whole guarantee.
    assert_eq!(
        googlebooks::isbn_url(&offline_config(None), ISBN13),
        format!("http://gb.test/books/v1/volumes?q=isbn:{ISBN13}")
    );
    assert_eq!(
        googlebooks::isbn_url(&offline_config(Some("sekret")), ISBN13),
        format!("http://gb.test/books/v1/volumes?q=isbn:{ISBN13}")
    );
}

/// Every URL this provider builds, with a key configured. Kept in one place so
/// a new builder is one line away from being covered by the property below.
fn all_google_books_urls(config: &MetadataLookupConfig) -> Vec<String> {
    vec![
        googlebooks::isbn_url(config, ISBN13),
        googlebooks::bare_url(config, ISBN13),
        googlebooks::search_url(config, &title_query("war & peace")).unwrap(),
        googlebooks::search_url(config, &title_query_isbn(ISBN13)).unwrap(),
        googlebooks::volume_url(config, "AQk_EAAAQBAJ").unwrap(),
    ]
}

#[test]
fn no_google_books_url_carries_the_api_key() {
    // AC1/AC3: the property, not the instances. `strip_url` used to be the only
    // thing between a failed request and a key in `omnibus.log`, and it was
    // opt-in per call site — so it was missing from all three `json()` sites
    // (#2129). With no key in any URL, a fallible step added later has nothing
    // to leak.
    for url in all_google_books_urls(&offline_config(Some("sekret"))) {
        assert!(!url.contains("sekret"), "api key in a request url: {url}");
        assert!(
            !url.contains("key="),
            "key parameter in a request url: {url}"
        );
    }
}

#[tokio::test]
async fn googlebooks_sends_the_key_as_a_header_and_not_in_the_url() {
    let server = MockServer::start().await;
    mount_gb(&server, gb_hit()).await;

    googlebooks::by_isbn(&keyed_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture volume must resolve");

    let requests = server
        .received_requests()
        .await
        .expect("the mock server records requests");
    let request = requests.first().expect("by_isbn must have asked");
    // The move is only safe if it still authenticates: assert the header
    // arrived, not merely that the URL is clean.
    assert_eq!(
        request
            .headers
            .get("x-goog-api-key")
            .and_then(|v| v.to_str().ok()),
        Some("k")
    );
    assert!(
        !request.url.as_str().contains("key="),
        "url must carry no credential: {}",
        request.url
    );
}

#[tokio::test]
async fn googlebooks_sends_no_key_header_when_none_is_configured() {
    // Keyless is a supported mode (the shared anonymous quota), so an empty or
    // literal-`None` header would be a request Google rejects outright.
    let server = MockServer::start().await;
    mount_gb(&server, gb_hit()).await;

    googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture volume must resolve");

    let requests = server
        .received_requests()
        .await
        .expect("the mock server records requests");
    let request = requests.first().expect("by_isbn must have asked");
    assert!(request.headers.get("x-goog-api-key").is_none());
}

#[tokio::test]
async fn googlebooks_failure_never_renders_the_api_key() {
    // Belt and braces over the header move: this is the error object both the
    // warn log and any terminal Provider error render, so it stays guarded even
    // though there is no longer a URL carrying the key for it to print. Call
    // the provider directly — under the key-dependent ladder a keyed Google
    // Books is the *primary*, so its error is swallowed to a warn and a full
    // `search_provider_by_isbn` would fall through to Open Library instead of
    // returning it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;

    let config = MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("super-secret-key".into()),
            ..ProviderKeys::default()
        },
        ..config_for(&server)
    };
    let err = googlebooks::by_isbn(&config, ISBN13).await.unwrap_err();
    // Walk the whole source chain — that's what `{:#}`/`{:?}` logging renders.
    let rendered = format!("{err:?} {err:#}");
    assert!(
        !rendered.contains("super-secret-key"),
        "api key leaked into the error chain: {rendered}"
    );
}

#[tokio::test]
async fn googlebooks_decode_failure_never_renders_the_api_key() {
    // AC2. The specific shape that leaked before #2129: a keyed instance that
    // has exhausted its quota gets 200 with an HTML page, `Response::json`
    // fails, and that decode error carries the request URL just as a transport
    // error does. Asserted rather than inspected, on every rung a decode can
    // fail: the ISBN query, the search, and the `by_ref` hydrate.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200).set_body_string("<html><title>Quota exceeded</title>"),
        )
        .mount(&server)
        .await;

    let config = MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("super-secret-key".into()),
            ..ProviderKeys::default()
        },
        ..config_for(&server)
    };
    let failures = [
        googlebooks::by_isbn(&config, ISBN13).await.unwrap_err(),
        googlebooks::search(&config, &title_query("effective java"))
            .await
            .unwrap_err(),
        googlebooks::by_ref(&config, "AQk_EAAAQBAJ")
            .await
            .unwrap_err(),
    ];
    for err in failures {
        let rendered = format!("{err:?} {err:#}");
        assert!(
            rendered.contains("not valid json"),
            "must report a decode failure, got: {rendered}"
        );
        assert!(
            !rendered.contains("super-secret-key"),
            "api key leaked into the error chain: {rendered}"
        );
    }
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

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
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

    let err = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
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

// ── Google Books bare-text fallback ──────────────────────────────
//
// Google's `isbn:` field search has been observed answering 200/totalItems=0
// for volumes the corpus holds (2026-08), while the same bare-text `q=<isbn>`
// query hits. The provider retries once as bare text before calling it a miss.

/// Mount a GB mock that only matches a specific `q` value, so the field and
/// bare queries can answer differently within one test. Expects exactly one
/// request — `server.verify()` then proves the query pattern actually ran.
async fn mount_gb_q(server: &MockServer, q: &str, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", q))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .expect(1)
        .mount(server)
        .await;
}

#[tokio::test]
async fn googlebooks_falls_back_to_bare_query_when_isbn_search_is_empty() {
    let server = MockServer::start().await;
    mount_gb_q(
        &server,
        &format!("isbn:{ISBN13}"),
        json!({ "totalItems": 0 }),
    )
    .await;
    mount_gb_q(&server, ISBN13, gb_hit()).await;

    let meta = googlebooks::by_isbn(&keyed_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("bare-text fallback must resolve the ISBN");
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
    assert_eq!(meta.title, "Effective Java");
    // Bare-text search may return a sibling edition; the meta must still carry
    // the *scanned* ISBN so a check-in stores the barcode that was scanned.
    assert_eq!(meta.isbn13.as_deref(), Some(ISBN13));
    server.verify().await;
}

#[tokio::test]
async fn googlebooks_lookup_skips_the_bare_query_without_a_key() {
    // Keyless requests share Google's anonymous daily quota (#1614), so a
    // field-search miss must stay a clean miss rather than doubling into a
    // second request.
    let server = MockServer::start().await;
    mount_gb_q(
        &server,
        &format!("isbn:{ISBN13}"),
        json!({ "totalItems": 0 }),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", ISBN13))
        .respond_with(ResponseTemplate::new(200).set_body_json(gb_hit()))
        .expect(0)
        .mount(&server)
        .await;

    let meta = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(meta.is_none(), "keyless miss must not issue a bare query");
    server.verify().await;
}

#[tokio::test]
async fn googlebooks_hit_never_issues_a_bare_query() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", format!("isbn:{ISBN13}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(gb_hit()))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", ISBN13))
        .respond_with(ResponseTemplate::new(200).set_body_json(gb_hit()))
        .expect(0)
        .mount(&server)
        .await;

    let meta = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(meta.is_some());
    server.verify().await;
}

#[tokio::test]
async fn googlebooks_double_miss_is_still_a_clean_miss() {
    let server = MockServer::start().await;
    mount_gb_q(
        &server,
        &format!("isbn:{ISBN13}"),
        json!({ "totalItems": 0 }),
    )
    .await;
    mount_gb_q(&server, ISBN13, json!({ "totalItems": 0 })).await;

    let meta = googlebooks::by_isbn(&keyed_config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(
        meta.is_none(),
        "double miss must be a clean miss, not an error"
    );
    server.verify().await;
}

#[tokio::test]
async fn googlebooks_bare_query_failure_degrades_to_a_clean_miss() {
    // The bare query is a bonus attempt: if it fails outright after the field
    // query answered cleanly, the lookup reports the miss it already had
    // rather than turning a would-be "unresolved" into a user-facing outage.
    let server = MockServer::start().await;
    mount_gb_q(
        &server,
        &format!("isbn:{ISBN13}"),
        json!({ "totalItems": 0 }),
    )
    .await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", ISBN13))
        .respond_with(ResponseTemplate::new(503))
        // A 503 is retryable, so the bare query burns the full retry budget.
        .expect(3)
        .mount(&server)
        .await;

    let meta = googlebooks::by_isbn(&keyed_config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(meta.is_none(), "bare-query failure must degrade to a miss");
    server.verify().await;
}

#[test]
fn googlebooks_bare_url_drops_the_field_restriction() {
    assert_eq!(
        googlebooks::bare_url(&offline_config(Some("sekret")), ISBN13),
        format!("http://gb.test/books/v1/volumes?q={ISBN13}")
    );
}

#[test]
fn publication_year_trims_google_dates_to_a_bare_year() {
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

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.year.as_deref(), Some("2025"));
    assert_eq!(meta.pages, None, "0 pages means unknown, not zero-length");
}

// ── the picker's fields: genres, print pages, ISBN-10 ────────────

#[tokio::test]
async fn googlebooks_lookup_populates_genres_print_pages_and_isbn10() {
    let server = MockServer::start().await;
    mount_gb(&server, gb_hit()).await;

    let edition = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture volume must resolve");
    assert_eq!(edition.genres, vec!["Computers", "Java"]);
    assert_eq!(edition.pages, Some(416));
    assert_eq!(edition.isbn10.as_deref(), Some(ISBN10));
}

#[tokio::test]
async fn googlebooks_leaves_the_new_fields_unset_when_the_volume_omits_them() {
    // A zero `pageCount` is Google's "unknown", and staging that over a real
    // count is the failure this guards: it must be `None`, not `Some(0)`.
    let server = MockServer::start().await;
    mount_gb(
        &server,
        json!({ "items": [{ "volumeInfo": {
            "title": "Effective Java",
            "pageCount": 0,
            "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
        }}]}),
    )
    .await;

    let edition = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture volume must resolve");
    assert!(edition.genres.is_empty());
    assert_eq!(edition.pages, None);
    assert_eq!(edition.isbn10, None);
}

#[tokio::test]
async fn googlebooks_drops_an_isbn10_that_names_a_different_printing() {
    // The bare-text fallback can surface a sibling edition, whose ISBN-10
    // would pair with an ISBN-13 that isn't the one being answered with.
    let server = MockServer::start().await;
    mount_gb(
        &server,
        json!({ "items": [{ "volumeInfo": {
            "title": "Effective Java",
            // 0141439513 is Pride and Prejudice's — a valid ISBN-10 for a
            // different book entirely.
            "industryIdentifiers": [{ "type": "ISBN_10", "identifier": "0141439513" }],
        }}]}),
    )
    .await;

    let edition = googlebooks::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture volume must resolve");
    assert_eq!(edition.isbn13.as_deref(), Some(ISBN13));
    assert_eq!(edition.isbn10, None);
}

#[tokio::test]
async fn a_swallowed_bare_text_failure_still_leaves_the_cooldown_it_recorded() {
    // `by_isbn` degrades a failed bare-text fallback to a clean miss, so a 429
    // can be recorded while the call still answers `Ok(None)`. Treating that
    // `Ok` as "the provider is fine" would erase the refusal we just learned.
    let server = MockServer::start().await;
    // The field-scoped query misses cleanly...
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", format!("isbn:{ISBN13}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "totalItems": 0 })))
        .mount(&server)
        .await;
    // ...and the bare-text fallback is refused.
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", ISBN13))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let config = keyed_config_for(&server);

    let found = providers::run(
        MetadataProvider::GoogleBooks,
        &config,
        &title_query_isbn(ISBN13),
    )
    .await
    .expect("the fallback's failure degrades to a miss");
    assert!(found.is_empty());
    assert!(
        config
            .throttle
            .remaining(MetadataProvider::GoogleBooks)
            .is_some(),
        "the refusal must outlive the Ok that swallowed it"
    );
}
