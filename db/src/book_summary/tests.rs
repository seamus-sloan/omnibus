//! Tests for the on-demand summary fetch: the OpenLibrary work-lookup client
//! (against `wiremock`) and the provider-injectable orchestrator.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::summary::SummarySource;

use super::fetch_summary_with;
use super::openlibrary::{self, OpenLibrarySummaryConfig};
use crate::pool::init_db;
use crate::suggestions::hardcover::HardcoverConfig;
use crate::test_support::seed_synced_ebook;

fn ol_config(server: &MockServer) -> OpenLibrarySummaryConfig {
    OpenLibrarySummaryConfig {
        base_url: server.uri(),
        timeout: std::time::Duration::from_secs(5),
    }
}

/// A Hardcover config whose base_url points nowhere useful — for tests that
/// exercise only the OpenLibrary branch and never make a Hardcover call.
fn unused_hardcover() -> HardcoverConfig {
    HardcoverConfig::new(String::new())
}

// ── OpenLibrary client (wiremock) ────────────────────────────────

#[tokio::test]
async fn openlibrary_fetch_resolves_isbn_then_reads_string_description() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/isbn/9780140328721.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "works": [{ "key": "/works/OL45804W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL45804W.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "description": "A clever fox outwits three farmers."
        })))
        .mount(&server)
        .await;

    let got = openlibrary::fetch(
        &ol_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        Some("Roald Dahl"),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("A clever fox outwits three farmers."));
}

#[tokio::test]
async fn openlibrary_fetch_normalizes_rich_text_description_object() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/isbn/9780140328721.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "works": [{ "key": "/works/OL45804W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL45804W.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "description": { "type": "/type/text", "value": "  Rich text blurb.  " }
        })))
        .mount(&server)
        .await;

    let got = openlibrary::fetch(
        &ol_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        None,
    )
    .await
    .unwrap();
    // The object's `value` is used, and surrounding whitespace is trimmed.
    assert_eq!(got.as_deref(), Some("Rich text blurb."));
}

#[tokio::test]
async fn openlibrary_fetch_falls_back_to_title_author_search_when_no_isbn() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "key": "/works/OL17352669W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL17352669W.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "description": "Feyre is dragged into a faerie land."
        })))
        .mount(&server)
        .await;

    let got = openlibrary::fetch(
        &ol_config(&server),
        &[],
        "A Court of Thorns and Roses",
        Some("Sarah J. Maas"),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("Feyre is dragged into a faerie land."));
}

#[tokio::test]
async fn openlibrary_fetch_returns_none_when_isbn_unknown_and_search_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/isbn/9999999999999.json"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [] })))
        .mount(&server)
        .await;

    let got = openlibrary::fetch(
        &ol_config(&server),
        &["9999999999999".to_string()],
        "Nonexistent Book",
        None,
    )
    .await
    .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn openlibrary_fetch_treats_blank_work_description_as_none() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/isbn/9780140328721.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "works": [{ "key": "/works/OL45804W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL45804W.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "description": "   " })))
        .mount(&server)
        .await;
    // A blank work description is treated as absent, so the fetch falls through
    // to the title search — which finds nothing here, yielding an overall miss.
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [] })))
        .mount(&server)
        .await;

    let got = openlibrary::fetch(
        &ol_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        None,
    )
    .await
    .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn openlibrary_fetch_errors_when_work_request_returns_500() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/isbn/9780140328721.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "works": [{ "key": "/works/OL45804W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL45804W.json"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let err = openlibrary::fetch(
        &ol_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        None,
    )
    .await
    .expect_err("a 500 on the work request must surface as an error");
    assert!(err.to_string().contains("open library work"));
}

// ── orchestrator (fetch_summary_with, via the pool) ──────────────

#[tokio::test]
async fn fetch_summary_with_openlibrary_resolves_via_search_for_a_seeded_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;
    // Seeded book has no ISBN, so the client goes straight to search.
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "key": "/works/OL1W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/works/OL1W.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "description": "The seeded book blurb."
        })))
        .mount(&server)
        .await;

    let got = fetch_summary_with(
        &pool,
        &uuid,
        SummarySource::OpenLibrary,
        &unused_hardcover(),
        &ol_config(&server),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("The seeded book blurb."));
}

#[tokio::test]
async fn fetch_summary_with_hardcover_resolves_by_title_then_reads_description() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;
    // 1. Title fallback resolves the book to a Hardcover id (no ISBN on the
    //    seeded book, so the ISBN pass is skipped).
    Mock::given(method("POST"))
        .and(body_string_contains("title: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 100, "slug": "src", "title": "Test Source",
                "contributions": [{ "author": { "name": "Source Author" } }],
                "book_series": [], "image": null
            }] }
        })))
        .mount(&server)
        .await;
    // 2. Description query for the resolved id.
    Mock::given(method("POST"))
        .and(body_string_contains("description"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{ "description": "Hardcover's blurb for the book." }] }
        })))
        .mount(&server)
        .await;

    let hc = HardcoverConfig {
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        timeout: std::time::Duration::from_secs(5),
    };
    let got = fetch_summary_with(
        &pool,
        &uuid,
        SummarySource::Hardcover,
        &hc,
        &OpenLibrarySummaryConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("Hardcover's blurb for the book."));
}

#[tokio::test]
async fn fetch_summary_with_hardcover_returns_none_when_book_unresolved() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;
    // Title fallback finds nothing → the book doesn't resolve → clean miss.
    Mock::given(method("POST"))
        .and(body_string_contains("title: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [] }
        })))
        .mount(&server)
        .await;

    let hc = HardcoverConfig {
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        timeout: std::time::Duration::from_secs(5),
    };
    let got = fetch_summary_with(
        &pool,
        &uuid,
        SummarySource::Hardcover,
        &hc,
        &OpenLibrarySummaryConfig::default(),
    )
    .await
    .unwrap();
    assert!(got.is_none());
}
