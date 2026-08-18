//! Fan-out edition search tests: attribution, un-collapsed cross-source
//! duplicates, the per-source status for each of the three outcomes, and the
//! explicit provider filter. Each provider is driven against a `wiremock`
//! server, mirroring the sibling ladder suite.

use std::time::Duration;

use omnibus_shared::metadata_lookup::{
    MetadataProvider, ProviderSearchStatus, EDITIONS_PER_PROVIDER,
};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::metadata_lookup::ProviderKeys;

use super::*;

const OL_SEARCH_PATH: &str = "/search.json";
const GB_PATH: &str = "/books/v1/volumes";
const QUERY: &str = "effective java";

/// Effective Java's ISBN-13, and a second edition's, so a cross-source
/// duplicate is distinguishable from two genuinely different printings.
const ISBN13: &str = "9780134685991";
const OTHER_ISBN13: &str = "9780321356680";

/// Every provider pointed at the mock server, keyless — Hardcover is
/// therefore unconfigured, which is the `NotConfigured` case.
fn config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: server.uri(),
        googlebooks_base: server.uri(),
        hardcover_base: server.uri(),
        keys: ProviderKeys::default(),
        timeout: Duration::from_secs(5),
    }
}

/// [`config_for`] with both keys set, so all three providers are configured.
fn fully_keyed_config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("gb-key".into()),
            hardcover: Some("hc-key".into()),
        },
        ..config_for(server)
    }
}

async fn mount_ol_search(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .and(query_param("title", QUERY))
        .respond_with(response)
        .mount(server)
        .await;
}

async fn mount_gb_search(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", QUERY))
        .respond_with(response)
        .mount(server)
        .await;
}

/// Hardcover speaks GraphQL over POST at the config's base url.
async fn mount_hc(server: &MockServer, response: ResponseTemplate) {
    Mock::given(method("POST"))
        .respond_with(response)
        .mount(server)
        .await;
}

fn ol_docs(isbns: &[&str]) -> serde_json::Value {
    json!({
        "docs": isbns.iter().enumerate().map(|(i, isbn)| json!({
            "title": format!("Effective Java {i}"),
            "author_name": ["Joshua Bloch"],
            "isbn": [isbn],
        })).collect::<Vec<_>>()
    })
}

fn gb_items(isbns: &[&str]) -> serde_json::Value {
    json!({
        "items": isbns.iter().enumerate().map(|(i, isbn)| json!({
            "volumeInfo": {
                "title": format!("Effective Java vol {i}"),
                "authors": ["Joshua Bloch"],
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": isbn }],
            }
        })).collect::<Vec<_>>()
    })
}

fn hc_books(isbn: &str) -> serde_json::Value {
    json!({ "data": { "books": [{
        "id": 42,
        "title": "Effective Java",
        "contributions": [{ "author": { "name": "Joshua Bloch" } }],
        "book_series": [],
        "editions": [{ "isbn_13": isbn }],
    }]}})
}

/// A valid ISBN-13 for `seq`, check digit included — `normalize_isbn` rejects
/// a bad one, so a bulk fixture can't just count upwards.
fn make_isbn13(seq: usize) -> String {
    let body = format!("978{seq:09}");
    let sum: u32 = body
        .bytes()
        .enumerate()
        .map(|(i, b)| {
            let digit = u32::from(b - b'0');
            if i % 2 == 0 {
                digit
            } else {
                digit * 3
            }
        })
        .sum();
    format!("{body}{}", (10 - sum % 10) % 10)
}

/// The status recorded for one provider, for terse assertions.
fn status_of(
    response: &EditionSearchResponse,
    provider: MetadataProvider,
) -> &ProviderSearchStatus {
    &response
        .sources
        .iter()
        .find(|s| s.provider == provider)
        .expect("every selected provider must appear in the report")
        .status
}

fn sources_named(response: &EditionSearchResponse) -> Vec<MetadataProvider> {
    response.sources.iter().map(|s| s.provider).collect()
}

// ── AC1: every provider answers, attributed, un-collapsed ────────

#[tokio::test]
async fn search_all_providers_returns_attributed_candidates_from_every_configured_provider() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[ISBN13])),
    )
    .await;
    mount_gb_search(
        &server,
        ResponseTemplate::new(200).set_body_json(gb_items(&[OTHER_ISBN13])),
    )
    .await;
    mount_hc(
        &server,
        ResponseTemplate::new(200).set_body_json(hc_books(ISBN13)),
    )
    .await;

    let response = search_all_providers(&fully_keyed_config_for(&server), QUERY, None).await;

    assert_eq!(response.editions.len(), 3);
    for provider in [
        MetadataProvider::OpenLibrary,
        MetadataProvider::GoogleBooks,
        MetadataProvider::Hardcover,
    ] {
        assert_eq!(
            status_of(&response, provider),
            &ProviderSearchStatus::Answered { count: 1 },
            "{provider:?} should have answered"
        );
        assert!(
            response.editions.iter().any(|e| e.source == provider),
            "{provider:?} should be attributed on a candidate"
        );
    }
}

#[tokio::test]
async fn search_all_providers_keeps_two_sources_sharing_an_isbn_as_two_entries() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[ISBN13])),
    )
    .await;
    mount_gb_search(
        &server,
        ResponseTemplate::new(200).set_body_json(gb_items(&[ISBN13])),
    )
    .await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    let same_isbn: Vec<_> = response
        .editions
        .iter()
        .filter(|e| e.isbn13 == ISBN13)
        .collect();
    assert_eq!(
        same_isbn.len(),
        2,
        "a shared ISBN across two sources must not collapse"
    );
    assert_ne!(same_isbn[0].source, same_isbn[1].source);
}

#[tokio::test]
async fn search_all_providers_attributes_a_provider_ref_to_every_candidate() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[ISBN13])),
    )
    .await;
    mount_gb_search(&server, ResponseTemplate::new(200).set_body_json(json!({}))).await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    let edition = response.editions.first().expect("open library answered");
    assert!(!edition.provider_ref.is_empty());
    assert_eq!(edition.provider_ref, edition.isbn13);
}

#[tokio::test]
async fn search_all_providers_caps_each_provider_at_the_per_provider_limit() {
    let server = MockServer::start().await;
    // Distinct ISBN-13s so nothing is dropped for being unusable; more than
    // the cap, so the cap is what trims the bucket.
    let isbns: Vec<String> = (0..(EDITIONS_PER_PROVIDER + 4)).map(make_isbn13).collect();
    let refs: Vec<&str> = isbns.iter().map(String::as_str).collect();
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&refs)),
    )
    .await;
    mount_gb_search(&server, ResponseTemplate::new(200).set_body_json(json!({}))).await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    let open_library = response
        .editions
        .iter()
        .filter(|e| e.source == MetadataProvider::OpenLibrary)
        .count();
    assert!(
        open_library <= EDITIONS_PER_PROVIDER,
        "one chatty source must not exceed its bucket: {open_library}"
    );
    assert_eq!(
        status_of(&response, MetadataProvider::OpenLibrary),
        &ProviderSearchStatus::Answered {
            count: open_library
        }
    );
}

// ── AC2: one provider failing still answers 200-shaped ───────────

#[tokio::test]
async fn search_all_providers_reports_failed_for_one_provider_and_results_for_the_others() {
    let server = MockServer::start().await;
    mount_ol_search(&server, ResponseTemplate::new(500)).await;
    mount_gb_search(
        &server,
        ResponseTemplate::new(200).set_body_json(gb_items(&[ISBN13])),
    )
    .await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    assert!(matches!(
        status_of(&response, MetadataProvider::OpenLibrary),
        ProviderSearchStatus::Failed { .. }
    ));
    assert_eq!(
        status_of(&response, MetadataProvider::GoogleBooks),
        &ProviderSearchStatus::Answered { count: 1 }
    );
    assert_eq!(response.editions.len(), 1);
}

#[tokio::test]
async fn search_all_providers_reports_every_source_failed_when_no_provider_answers() {
    let server = MockServer::start().await;
    mount_ol_search(&server, ResponseTemplate::new(500)).await;
    mount_gb_search(&server, ResponseTemplate::new(500)).await;
    mount_hc(&server, ResponseTemplate::new(500)).await;

    let response = search_all_providers(&fully_keyed_config_for(&server), QUERY, None).await;

    assert!(response.editions.is_empty());
    assert_eq!(response.sources.len(), 3);
    for source in &response.sources {
        assert!(
            matches!(source.status, ProviderSearchStatus::Failed { .. }),
            "{:?} should be Failed, got {:?}",
            source.provider,
            source.status
        );
    }
}

#[tokio::test]
async fn search_all_providers_never_puts_an_api_key_in_a_failure_message() {
    let server = MockServer::start().await;
    // Google Books retries a 429 and then reports the status; the key rides
    // in the request URL, which must never reach the message.
    mount_gb_search(&server, ResponseTemplate::new(429)).await;
    mount_ol_search(&server, ResponseTemplate::new(500)).await;
    let mut config = fully_keyed_config_for(&server);
    config.timeout = Duration::from_millis(500);

    let response = search_all_providers(&config, QUERY, None).await;

    for source in &response.sources {
        if let ProviderSearchStatus::Failed { message } = &source.status {
            assert!(!message.contains("gb-key"), "leaked key: {message}");
            assert!(!message.contains("hc-key"), "leaked key: {message}");
        }
    }
}

// ── AC3: unconfigured providers are reported, never asked ────────

#[tokio::test]
async fn search_all_providers_reports_not_configured_without_sending_a_request() {
    let server = MockServer::start().await;
    mount_ol_search(&server, ResponseTemplate::new(200).set_body_json(json!({}))).await;
    mount_gb_search(&server, ResponseTemplate::new(200).set_body_json(json!({}))).await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    assert_eq!(
        status_of(&response, MetadataProvider::Hardcover),
        &ProviderSearchStatus::NotConfigured
    );
    // Only the two catalog GETs were issued; a Hardcover request would have
    // been a third (and, unmounted, a 404 reported as `Failed`).
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        2,
        "an unconfigured provider must never be sent a request"
    );
}

// ── AC4: the explicit provider filter ────────────────────────────

#[tokio::test]
async fn search_all_providers_queries_only_the_providers_named_in_the_filter() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[ISBN13])),
    )
    .await;
    mount_gb_search(
        &server,
        ResponseTemplate::new(200).set_body_json(gb_items(&[OTHER_ISBN13])),
    )
    .await;

    let response = search_all_providers(
        &config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary]),
    )
    .await;

    assert_eq!(
        sources_named(&response),
        vec![MetadataProvider::OpenLibrary]
    );
    assert!(response
        .editions
        .iter()
        .all(|e| e.source == MetadataProvider::OpenLibrary));
}

#[tokio::test]
async fn search_all_providers_ignores_a_repeated_filter_entry() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[ISBN13])),
    )
    .await;

    let response = search_all_providers(
        &config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary, MetadataProvider::OpenLibrary]),
    )
    .await;

    assert_eq!(
        sources_named(&response),
        vec![MetadataProvider::OpenLibrary]
    );
    assert_eq!(response.editions.len(), 1);
}

#[tokio::test]
async fn search_all_providers_returns_an_empty_report_for_an_empty_filter() {
    let server = MockServer::start().await;

    let response = search_all_providers(&config_for(&server), QUERY, Some(&[])).await;

    assert!(response.editions.is_empty());
    assert!(response.sources.is_empty());
}

// ── ordering ─────────────────────────────────────────────────────

#[tokio::test]
async fn search_all_providers_interleaves_buckets_so_one_source_cannot_lead_the_whole_list() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        ResponseTemplate::new(200).set_body_json(ol_docs(&[
            "9780134685991",
            "9780321356680",
            "9780132350884",
        ])),
    )
    .await;
    mount_gb_search(
        &server,
        ResponseTemplate::new(200).set_body_json(gb_items(&["9780201633610"])),
    )
    .await;

    let response = search_all_providers(&config_for(&server), QUERY, None).await;

    assert_eq!(response.editions.len(), 4);
    let google_at = response
        .editions
        .iter()
        .position(|e| e.source == MetadataProvider::GoogleBooks)
        .expect("google books answered");
    assert!(
        google_at < 2,
        "the quieter source must appear near the head, was at {google_at}"
    );
}
