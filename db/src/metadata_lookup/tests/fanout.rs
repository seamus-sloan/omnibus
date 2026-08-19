//! Fan-out search tests: every configured provider asked at once, answers kept
//! attributed and un-collapsed, and a per-source status that tells "no
//! results" apart from "not configured" and "couldn't reach it". The ladder's
//! own first-answer-wins behaviour lives in the parent module.

use omnibus_shared::metadata_lookup::{MetadataProvider, ProviderSearchStatus};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::*;
use super::{config_for, with_check_digit, GB_PATH, ISBN13, OL_SEARCH_PATH, QUERY};

/// Hardcover's GraphQL endpoint is the config's base URL itself, so it lands
/// on the mock server's root path.
const HC_PATH: &str = "/";

/// Both optional keys set, so every provider in the catalog is `configured`
/// and the fan-out asks all three.
fn all_keyed_config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: Some("gb-key".into()),
            hardcover: Some("hc-key".into()),
        },
        ..config_for(server)
    }
}

// The three providers deliberately answer with the *same* ISBN under
// different titles: AC1's "two sources, one ISBN, two candidates".

fn ol_docs(count: usize) -> serde_json::Value {
    let docs: Vec<serde_json::Value> = (0..count)
        .map(|i| {
            json!({
                "key": format!("/works/OL{i}W"),
                "title": format!("Effective Java (OL {i})"),
                "author_name": ["Joshua Bloch"],
                "isbn": [if i == 0 { ISBN13.to_string() } else { with_check_digit(&format!("9780000000{i:02}")) }],
            })
        })
        .collect();
    json!({ "docs": docs })
}

fn gb_items() -> serde_json::Value {
    json!({
        "totalItems": 2,
        "items": [
            { "id": "gb-volume-1", "volumeInfo": {
                "title": "Effective Java (GB)",
                "authors": ["Joshua Bloch"],
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
            }},
            // Google answers repeat editions of one ISBN; the picker exists to
            // show them, so the fan-out must not collapse them.
            { "id": "gb-volume-2", "volumeInfo": {
                "title": "Effective Java (GB reprint)",
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
            }},
        ]
    })
}

fn hc_books() -> serde_json::Value {
    json!({ "data": { "books": [{
        "id": 4242,
        "title": "Effective Java (HC)",
        "contributions": [{ "author": { "name": "Joshua Bloch" } }],
        "editions": [{ "isbn_13": ISBN13 }],
    }]}})
}

async fn mount(server: &MockServer, verb: &str, at: &str, body: serde_json::Value) {
    Mock::given(method(verb))
        .and(path(at))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

async fn mount_status(server: &MockServer, verb: &str, at: &str, status: u16) {
    Mock::given(method(verb))
        .and(path(at))
        .respond_with(ResponseTemplate::new(status))
        .mount(server)
        .await;
}

/// Mount all three providers on their happy paths.
async fn mount_all(server: &MockServer) {
    mount(server, "GET", OL_SEARCH_PATH, ol_docs(1)).await;
    mount(server, "GET", GB_PATH, gb_items()).await;
    mount(server, "POST", HC_PATH, hc_books()).await;
}

/// The status recorded for one provider, or `None` if it has no row at all.
fn status_of(
    response: &omnibus_shared::metadata_lookup::EditionSearchResponse,
    provider: MetadataProvider,
) -> Option<&ProviderSearchStatus> {
    response
        .sources
        .iter()
        .find(|s| s.provider == provider)
        .map(|s| &s.status)
}

// ── AC1: attributed, un-collapsed candidates from every provider ─

#[tokio::test]
async fn search_all_providers_returns_attributed_candidates_from_every_configured_provider() {
    let server = MockServer::start().await;
    mount_all(&server).await;

    let found = search_all_providers(&all_keyed_config_for(&server), QUERY, None).await;

    assert_eq!(found.sources.len(), 3, "every provider gets a status row");
    for provider in [
        MetadataProvider::OpenLibrary,
        MetadataProvider::GoogleBooks,
        MetadataProvider::Hardcover,
    ] {
        assert!(
            found.editions.iter().any(|e| e.source == provider),
            "{provider:?} contributed no candidate: {:?}",
            found.editions
        );
    }
    // One ISBN, three sources, four candidates — nothing is collapsed.
    assert!(found.editions.iter().all(|e| e.isbn13 == ISBN13));
    assert_eq!(found.editions.len(), 4);
    assert_eq!(
        status_of(&found, MetadataProvider::GoogleBooks),
        Some(&ProviderSearchStatus::Answered { count: 2 }),
        "the two Google volumes of one ISBN stay two candidates"
    );
    assert_eq!(
        status_of(&found, MetadataProvider::OpenLibrary),
        Some(&ProviderSearchStatus::Answered { count: 1 })
    );
    assert_eq!(
        status_of(&found, MetadataProvider::Hardcover),
        Some(&ProviderSearchStatus::Answered { count: 1 })
    );
}

#[tokio::test]
async fn search_all_providers_carries_each_providers_own_handle_on_every_candidate() {
    let server = MockServer::start().await;
    mount_all(&server).await;

    let found = search_all_providers(&all_keyed_config_for(&server), QUERY, None).await;

    let refs = |provider| -> Vec<String> {
        found
            .editions
            .iter()
            .filter(|e| e.source == provider)
            .map(|e| e.provider_ref.clone())
            .collect()
    };
    assert_eq!(refs(MetadataProvider::OpenLibrary), vec!["/works/OL0W"]);
    assert_eq!(
        refs(MetadataProvider::GoogleBooks),
        vec!["gb-volume-1", "gb-volume-2"],
        "candidates sharing an ISBN are still told apart by their handles"
    );
    assert_eq!(refs(MetadataProvider::Hardcover), vec!["4242"]);
}

#[tokio::test]
async fn search_all_providers_falls_back_to_the_isbn_handle_when_a_row_carries_no_id() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [{ "title": "Effective Java", "isbn": [ISBN13] }] }),
    )
    .await;

    let found = search_all_providers(
        &config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary]),
    )
    .await;

    assert_eq!(found.editions[0].provider_ref, format!("isbn:{ISBN13}"));
}

// ── AC2: one provider down never fails the search ────────────────

#[tokio::test]
async fn search_all_providers_reports_a_failed_provider_and_still_returns_the_others() {
    let server = MockServer::start().await;
    mount(&server, "GET", OL_SEARCH_PATH, ol_docs(1)).await;
    mount_status(&server, "GET", GB_PATH, 500).await;
    mount(&server, "POST", HC_PATH, hc_books()).await;

    let found = search_all_providers(&all_keyed_config_for(&server), QUERY, None).await;

    assert!(
        matches!(
            status_of(&found, MetadataProvider::GoogleBooks),
            Some(ProviderSearchStatus::Failed { .. })
        ),
        "a 500 must read as Failed, not as an empty answer: {:?}",
        status_of(&found, MetadataProvider::GoogleBooks)
    );
    assert_eq!(found.editions.len(), 2, "the other two still answered");
    assert!(!found
        .editions
        .iter()
        .any(|e| e.source == MetadataProvider::GoogleBooks));
}

#[tokio::test]
async fn search_all_providers_reports_every_provider_failed_without_erroring() {
    let server = MockServer::start().await;
    mount_status(&server, "GET", OL_SEARCH_PATH, 500).await;
    mount_status(&server, "GET", GB_PATH, 500).await;
    mount_status(&server, "POST", HC_PATH, 500).await;

    let found = search_all_providers(&all_keyed_config_for(&server), QUERY, None).await;

    assert!(found.editions.is_empty());
    assert_eq!(found.sources.len(), 3);
    assert!(
        found
            .sources
            .iter()
            .all(|s| matches!(s.status, ProviderSearchStatus::Failed { .. })),
        "a total outage is still a 200 with three Failed rows: {:?}",
        found.sources
    );
}

#[tokio::test]
async fn search_all_providers_failure_message_never_renders_the_api_key() {
    // The Google Books key rides in the query string and a `reqwest::Error`
    // renders the request URL in its Display, so the status message is a leak
    // surface — it is served to any edit-permitted user.
    let server = MockServer::start().await;
    mount_status(&server, "GET", GB_PATH, 429).await;

    let found = search_all_providers(
        &all_keyed_config_for(&server),
        QUERY,
        Some(&[MetadataProvider::GoogleBooks]),
    )
    .await;

    let ProviderSearchStatus::Failed { message } =
        status_of(&found, MetadataProvider::GoogleBooks).unwrap()
    else {
        panic!("a 429 must read as Failed: {:?}", found.sources);
    };
    assert!(!message.contains("gb-key"), "got: {message}");
    assert!(!message.contains("key="), "got: {message}");
}

// ── AC3: an unconfigured provider is reported, never asked ───────

#[tokio::test]
async fn search_all_providers_reports_not_configured_without_sending_a_request() {
    let server = MockServer::start().await;
    mount(&server, "GET", OL_SEARCH_PATH, ol_docs(1)).await;
    mount(&server, "GET", GB_PATH, gb_items()).await;
    // Hardcover is deliberately left unmounted *and* unkeyed: a request would
    // 404 and surface as Failed, so NotConfigured proves none was sent.
    let found = search_all_providers(&config_for(&server), QUERY, None).await;

    assert_eq!(
        status_of(&found, MetadataProvider::Hardcover),
        Some(&ProviderSearchStatus::NotConfigured)
    );
    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        !requests.iter().any(|r| r.url.path() == HC_PATH),
        "an unconfigured provider must never be sent a request"
    );
}

// ── AC4: an explicit provider list is honoured ───────────────────

#[tokio::test]
async fn search_all_providers_asks_only_the_named_providers() {
    let server = MockServer::start().await;
    mount_all(&server).await;

    let found = search_all_providers(
        &all_keyed_config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary]),
    )
    .await;

    assert_eq!(found.sources.len(), 1);
    assert_eq!(found.sources[0].provider, MetadataProvider::OpenLibrary);
    assert_eq!(found.sources[0].display_name, "Open Library");
    assert!(found
        .editions
        .iter()
        .all(|e| e.source == MetadataProvider::OpenLibrary));

    let requests = server.received_requests().await.unwrap_or_default();
    assert!(
        requests.iter().all(|r| r.url.path() == OL_SEARCH_PATH),
        "an unnamed provider must not be asked"
    );
}

// ── per-provider cap ─────────────────────────────────────────────

#[tokio::test]
async fn search_all_providers_caps_what_one_provider_contributes() {
    // The mock ignores the `limit` the provider asked for, standing in for a
    // source that answers with more than it was asked for.
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        ol_docs(FANOUT_PROVIDER_LIMIT + 5),
    )
    .await;

    let found = search_all_providers(
        &config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary]),
    )
    .await;

    assert_eq!(found.editions.len(), FANOUT_PROVIDER_LIMIT);
    assert_eq!(
        status_of(&found, MetadataProvider::OpenLibrary),
        Some(&ProviderSearchStatus::Answered {
            count: FANOUT_PROVIDER_LIMIT
        }),
        "the reported count must match what actually reached the list"
    );
}

#[tokio::test]
async fn search_all_providers_reports_an_empty_answer_as_answered_zero() {
    let server = MockServer::start().await;
    mount(&server, "GET", OL_SEARCH_PATH, json!({ "docs": [] })).await;

    let found = search_all_providers(
        &config_for(&server),
        QUERY,
        Some(&[MetadataProvider::OpenLibrary]),
    )
    .await;

    assert!(found.editions.is_empty());
    assert_eq!(
        status_of(&found, MetadataProvider::OpenLibrary),
        Some(&ProviderSearchStatus::Answered { count: 0 }),
        "a clean miss is an answer, not a failure"
    );
}
