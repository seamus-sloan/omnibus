//! Open Library provider tests: the parse-failure path and the best-effort
//! enrichment lookup (series, first-publish year) that fills the fields no
//! other provider carries. Ladder-level ordering lives in the parent module.

use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};

use super::super::providers::openlibrary;
use super::super::*;
use super::{config_for, gb_hit, mount_gb, mount_ol, ol_hit, ISBN13, OL_PATH};

// ── invalid response bodies ──────────────────────────────────────

#[tokio::test]
async fn openlibrary_lookup_errors_when_response_body_is_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(OL_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = openlibrary::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not valid json"),
        "must report a json parse failure, got: {err}"
    );
}

// ── Open Library enrichment (series, first-publish year) ─────────

/// Mount the two enrichment endpoints: the edition record (series) and the
/// search API's `isbn:` field query (first-publish year).
async fn mount_enrichment(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(format!("/isbn/{ISBN13}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "series": ["Addison-Wesley Java series"],
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .and(query_param("q", format!("isbn:{ISBN13}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "first_publish_year": 2001 }],
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn search_by_isbn_enriches_a_hit_with_series_and_first_publish_year() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    mount_enrichment(&server).await;

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.series.as_deref(), Some("Addison-Wesley Java series"));
    assert_eq!(meta.first_publish_year, Some(2001));
    // The edition's own date stays what the provider reported.
    assert_eq!(meta.year.as_deref(), Some("2018"));
}

#[tokio::test]
async fn search_by_isbn_enriches_a_google_books_hit_too() {
    // Enrichment keys on the ISBN alone, so a Google Books resolution still
    // gets Open Library's series / first-publish fields.
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, gb_hit()).await;
    mount_enrichment(&server).await;

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::GoogleBooks);
    assert_eq!(meta.series.as_deref(), Some("Addison-Wesley Java series"));
    assert_eq!(meta.first_publish_year, Some(2001));
}

#[tokio::test]
async fn search_by_isbn_survives_enrichment_failure_with_fields_unset() {
    // No enrichment endpoints mounted: both GETs 404. The lookup must still
    // resolve — enrichment is strictly best-effort.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.title, "Effective Java");
    assert_eq!(meta.series, None);
    assert_eq!(meta.first_publish_year, None);
}

#[tokio::test]
async fn openlibrary_enrich_drops_a_blank_or_oversized_series() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/isbn/{ISBN13}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "series": ["   ", "x".repeat(ExternalBookMeta::NAME_MAX_LEN + 1), "Real Series"],
        })))
        .mount(&server)
        .await;

    let enrichment = openlibrary_enrich(&config_for(&server), ISBN13).await;
    // Blank and oversized statements are skipped, not truncated — the meta is
    // posted back on the write paths, where `validate` would reject them.
    assert_eq!(enrichment.series.as_deref(), Some("Real Series"));
    assert_eq!(enrichment.first_publish_year, None);
}
