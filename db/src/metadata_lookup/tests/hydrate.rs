//! Hydrate-on-select tests: the second call the picker makes once a
//! candidate is chosen. The behaviour under test is that a re-fetch adds the
//! detail record's fields — chiefly Open Library's description, which no
//! search surface carries — without ever costing the candidate one it had.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::*;
use super::{config_for, gb_hit, mount_gb, mount_ol, ol_hit, ISBN13};

const WORK_KEY: &str = "/works/OL1W";

async fn mount_work(server: &MockServer, body: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path(format!("{WORK_KEY}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Open Library's enrichment lookups (`/isbn/<isbn>.json` and `search.json`)
/// answer 404 unless a test mounts them; `get_json_best_effort` degrades
/// those to `None`, which is the shape production sees for a record Open
/// Library only partly knows.
#[tokio::test]
async fn hydrate_edition_fills_open_librarys_description_from_the_selected_record() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_work(
        &server,
        json!({ "description": { "type": "/type/text", "value": "  The definitive guide.  " } }),
    )
    .await;

    let hydrated = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        WORK_KEY,
        ISBN13,
    )
    .await
    .unwrap()
    .unwrap();

    assert_eq!(
        hydrated.description.as_deref(),
        Some("The definitive guide.")
    );
    // The detail record's own fields survive the merge.
    assert_eq!(hydrated.publisher.as_deref(), Some("Addison-Wesley"));
    assert_eq!(hydrated.pages, Some(416));
    // Echoed back, not re-minted: the picker keys its selection on this.
    assert_eq!(hydrated.provider_ref, WORK_KEY);
}

#[tokio::test]
async fn hydrate_edition_reads_open_librarys_bare_string_description_form() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_work(&server, json!({ "description": "A plain string." })).await;

    let hydrated = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        WORK_KEY,
        ISBN13,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(hydrated.description.as_deref(), Some("A plain string."));
}

#[tokio::test]
async fn hydrate_edition_skips_the_record_fetch_for_a_ref_that_is_not_a_record_path() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    // `.expect(0)` is the assertion: a record mock *is* mounted and must go
    // unused, so this fails if the shape guard stops refusing the `isbn:`
    // fallback ref — which asserting `description == None` alone would not,
    // since a fetch that 404s produces the same `None`.
    Mock::given(method("GET"))
        .and(path(format!("{WORK_KEY}.json")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "description": "must never be read for a non-record ref" })),
        )
        .expect(0)
        .mount(&server)
        .await;

    let hydrated = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        &format!("isbn:{ISBN13}"),
        ISBN13,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(hydrated.description, None);
    assert_eq!(hydrated.provider_ref, format!("isbn:{ISBN13}"));
    // Explicit rather than left to drop: a mounted-but-unused expectation is
    // only checked when the server is torn down.
    server.verify().await;
}

#[tokio::test]
async fn hydrate_edition_never_fetches_a_record_for_a_ref_that_would_rewrite_the_host() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    // The base URL is *concatenated* with the ref, so `@host/x` would parse
    // with the configured base as userinfo and `evil.example` as the host.
    // Nothing must be requested for any of these.
    for hostile in [
        "@evil.example/x",
        "https://evil.example/x",
        "/works/../../../search",
    ] {
        let hydrated = hydrate_edition(
            &config_for(&server),
            MetadataProvider::OpenLibrary,
            hostile,
            ISBN13,
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(
            hydrated.description, None,
            "{hostile} must not resolve to a record"
        );
    }
    // Only the ISBN lookups the hydrate itself makes; no record fetch.
    let record_requests = server
        .received_requests()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.url.path().contains("/works/") || r.url.path().contains("/books/"))
        .count();
    assert_eq!(record_requests, 0);
}

#[tokio::test]
async fn hydrate_edition_returns_the_providers_own_record_for_google_books() {
    let server = MockServer::start().await;
    mount_gb(&server, gb_hit()).await;

    let hydrated = hydrate_edition(
        &config_for(&server),
        MetadataProvider::GoogleBooks,
        "gb-volume-1",
        ISBN13,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(hydrated.source, MetadataProvider::GoogleBooks);
    assert_eq!(
        hydrated.description.as_deref(),
        Some("The definitive guide.")
    );
    assert_eq!(hydrated.provider_ref, "gb-volume-1");
}

#[tokio::test]
async fn hydrate_edition_is_none_when_the_provider_no_longer_knows_the_isbn() {
    let server = MockServer::start().await;
    // An empty `jscmd=data` map is Open Library's "never heard of it".
    mount_ol(&server, json!({})).await;

    let hydrated = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        WORK_KEY,
        ISBN13,
    )
    .await
    .unwrap();
    assert!(hydrated.is_none());
}

#[tokio::test]
async fn hydrate_edition_rejects_a_malformed_isbn_before_asking_any_provider() {
    let server = MockServer::start().await;
    let err = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        WORK_KEY,
        "not-an-isbn",
    )
    .await
    .expect_err("a malformed ISBN is an input error, not a miss");
    assert!(matches!(err, MetadataLookupError::Isbn(_)));
}

#[tokio::test]
async fn hydrate_edition_surfaces_a_provider_failure_rather_than_reporting_a_miss() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/books"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let err = hydrate_edition(
        &config_for(&server),
        MetadataProvider::OpenLibrary,
        WORK_KEY,
        ISBN13,
    )
    .await
    .expect_err("a 500 from the provider is not the same as an unknown ISBN");
    assert!(matches!(err, MetadataLookupError::Provider(_)));
}
