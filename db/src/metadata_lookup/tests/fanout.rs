//! Fan-out search tests: every configured provider asked at once, answers kept
//! attributed and un-collapsed, and a per-source status that tells "no
//! results" apart from "not configured" and "couldn't reach it". The ladder's
//! own first-answer-wins behaviour lives in the parent module.

use std::time::{Duration, Instant};

use omnibus_shared::metadata_lookup::{MetadataProvider, ProviderSearchStatus};
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::*;
use super::{
    all_keyed_config_for, config_for, title_query, with_check_digit, GB_PATH, HC_PATH, ISBN13,
    OL_SEARCH_PATH, QUERY,
};

// The two catalogs deliberately answer with the *same* ISBN under different
// titles: AC1's "two sources, one ISBN, two candidates". Hardcover answers
// through its `search` endpoint, which describes works rather than printings,
// so its candidate carries no ISBN at all — and is still a candidate.

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

/// Hardcover's fan-out answer, which comes from its `search` endpoint and so
/// has the hit-document shape — not the `books(where:)` row shape the ISBN and
/// by-handle paths use.
fn hc_books() -> serde_json::Value {
    json!({ "data": { "search": { "results": { "found": 1, "hits": [{ "document": {
        "id": "4242",
        "slug": "effective-java",
        "title": "Effective Java (HC)",
        "author_names": ["Joshua Bloch"],
        "genres": ["Programming"],
        // No ISBN: a search document describes a work, whose `isbns` span
        // every edition, so none of them names this candidate's printing.
        "isbns": [ISBN13],
    }}]}}}})
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

    let found =
        search_all_providers(&all_keyed_config_for(&server), &title_query(QUERY), None).await;

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
    // The two catalogs answer with editions and so carry the shared ISBN;
    // Hardcover's search document describes the *work*, and says so by
    // carrying none rather than borrowing one of the work's other printings'.
    assert!(found
        .editions
        .iter()
        .filter(|e| e.source != MetadataProvider::Hardcover)
        .all(|e| e.isbn13.as_deref() == Some(ISBN13)));
    assert!(found
        .editions
        .iter()
        .filter(|e| e.source == MetadataProvider::Hardcover)
        .all(|e| e.isbn13.is_none()));
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

    let found =
        search_all_providers(&all_keyed_config_for(&server), &title_query(QUERY), None).await;

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
        &title_query(QUERY),
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

    let found =
        search_all_providers(&all_keyed_config_for(&server), &title_query(QUERY), None).await;

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

    let found =
        search_all_providers(&all_keyed_config_for(&server), &title_query(QUERY), None).await;

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
        &title_query(QUERY),
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
    let found = search_all_providers(&config_for(&server), &title_query(QUERY), None).await;

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
        &title_query(QUERY),
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
        &title_query(QUERY),
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
        &title_query(QUERY),
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

// ── relevance: what a provider returns is not what the picker shows ─

#[tokio::test]
async fn search_all_providers_drops_a_study_guide_rather_than_ranking_it_lower() {
    // Providers rank for their own purposes: ask any catalog for a famous
    // novel and it hands back books *about* the novel. Ordering alone leaves
    // them on screen.
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [
            { "key": "/works/OL1W", "title": "A Study Guide for Dune", "isbn": [ISBN13] },
            { "key": "/works/OL2W", "title": "Dune", "isbn": [with_check_digit("978000000002")] },
        ]}),
    )
    .await;
    mount(&server, "GET", GB_PATH, json!({ "totalItems": 0 })).await;

    let found = search_all_providers(&config_for(&server), &title_query("Dune"), None).await;
    let titles: Vec<&str> = found.editions.iter().map(|e| e.title.as_str()).collect();
    assert_eq!(titles, vec!["Dune"]);
}

#[tokio::test]
async fn search_all_providers_stamps_each_candidate_with_its_relevance() {
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [{ "key": "/works/OL1W", "title": "Dune", "isbn": [ISBN13] }]}),
    )
    .await;
    mount(&server, "GET", GB_PATH, json!({ "totalItems": 0 })).await;

    let found = search_all_providers(&config_for(&server), &title_query("Dune"), None).await;
    // Hundredths of a point: an exact title match scores 10.
    assert_eq!(found.editions[0].relevance, Some(1000));
}

#[tokio::test]
async fn search_all_providers_keeps_a_candidate_that_carries_no_isbn() {
    // The gate this change removes. `search.json` answers works, and a work
    // Open Library has not catalogued an ISBN for is still a book someone is
    // editing — it used to be dropped silently.
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [{ "key": "/works/OL9W", "title": "Beowulf" }]}),
    )
    .await;
    mount(&server, "GET", GB_PATH, json!({ "totalItems": 0 })).await;

    let found = search_all_providers(&config_for(&server), &title_query("Beowulf"), None).await;
    assert_eq!(found.editions.len(), 1, "got {:?}", found.editions);
    assert_eq!(found.editions[0].isbn13, None);
    assert_eq!(
        found.editions[0].provider_ref, "/works/OL9W",
        "the handle, not the ISBN, is what identifies a candidate"
    );
}

#[tokio::test]
async fn search_all_providers_drops_a_candidate_with_neither_an_isbn_nor_a_handle() {
    // Nothing to re-fetch it by and nothing to identify it with: showing it
    // would offer a row that cannot be selected.
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [{ "title": "Beowulf" }]}),
    )
    .await;
    mount(&server, "GET", GB_PATH, json!({ "totalItems": 0 })).await;

    let found = search_all_providers(&config_for(&server), &title_query("Beowulf"), None).await;
    assert!(found.editions.is_empty(), "got {:?}", found.editions);
}

// ── throttling ──────────────────────────────────────────────────

#[tokio::test]
async fn search_all_providers_reports_a_throttled_provider_without_asking_it() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    let config = all_keyed_config_for(&server);
    // Stand in for an earlier 429 from Open Library.
    // Above the schedule's first step, since `Retry-After` is a floor.
    config.throttle.record(
        MetadataProvider::OpenLibrary,
        Some(Duration::from_secs(600)),
    );

    let found = search_all_providers(&config, &title_query(QUERY), None).await;
    assert!(matches!(
        status_of(&found, MetadataProvider::OpenLibrary),
        Some(ProviderSearchStatus::Throttled { retry_after_secs }) if *retry_after_secs == 600
    ));
    assert!(
        !found
            .editions
            .iter()
            .any(|e| e.source == MetadataProvider::OpenLibrary),
        "a cooling-down provider contributes nothing"
    );
    // And the others are untouched by it.
    assert!(matches!(
        status_of(&found, MetadataProvider::GoogleBooks),
        Some(ProviderSearchStatus::Answered { .. })
    ));
}

#[tokio::test]
async fn search_all_providers_records_a_429_so_the_next_search_skips_the_provider() {
    let server = MockServer::start().await;
    mount(&server, "GET", OL_SEARCH_PATH, ol_docs(1)).await;
    mount_status(&server, "GET", GB_PATH, 429).await;
    let config = config_for(&server);

    let first = search_all_providers(&config, &title_query(QUERY), None).await;
    assert!(
        matches!(
            status_of(&first, MetadataProvider::GoogleBooks),
            Some(ProviderSearchStatus::Failed { .. })
        ),
        "the first search reports the refusal it actually got"
    );

    let second = search_all_providers(&config, &title_query(QUERY), None).await;
    assert!(
        matches!(
            status_of(&second, MetadataProvider::GoogleBooks),
            Some(ProviderSearchStatus::Throttled { .. })
        ),
        "the second must not walk back into the same 429"
    );
}

#[tokio::test]
async fn search_all_providers_clears_a_cooldown_once_a_provider_answers_again() {
    let server = MockServer::start().await;
    mount_all(&server).await;
    let config = all_keyed_config_for(&server);
    // A cooldown that has already lapsed must not suppress the next ask.
    config
        .throttle
        .record_at(MetadataProvider::GoogleBooks, None, Instant::now());
    config.throttle.clear(MetadataProvider::GoogleBooks);

    let found = search_all_providers(&config, &title_query(QUERY), None).await;
    assert!(matches!(
        status_of(&found, MetadataProvider::GoogleBooks),
        Some(ProviderSearchStatus::Answered { .. })
    ));
}

#[tokio::test]
async fn search_all_providers_returns_the_merged_list_already_ordered_by_score() {
    // The raw fan-out answers provider by provider. A client that just renders
    // what it receives should still see the best match first.
    let server = MockServer::start().await;
    mount(
        &server,
        "GET",
        OL_SEARCH_PATH,
        json!({ "docs": [{ "key": "/works/OL1W", "title": "Dune Messiah", "isbn": [ISBN13] }]}),
    )
    .await;
    mount(
        &server,
        "GET",
        GB_PATH,
        json!({ "items": [{ "id": "gb-1", "volumeInfo": {
            "title": "Dune",
            "industryIdentifiers": [{ "type": "ISBN_13", "identifier": with_check_digit("978000000002") }],
        }}]}),
    )
    .await;

    let found = search_all_providers(&config_for(&server), &title_query("Dune"), None).await;
    assert_eq!(
        found.editions[0].title, "Dune",
        "the exact match must lead even though its provider answered second"
    );
    assert_eq!(found.editions[0].source, MetadataProvider::GoogleBooks);
}
