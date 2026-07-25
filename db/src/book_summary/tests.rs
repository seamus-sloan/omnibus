//! Tests for the on-demand summary fetch: the OpenLibrary work-lookup client
//! (against `wiremock`) and the provider-injectable orchestrator.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::summary::SummarySource;

use super::googlebooks::{self, GoogleBooksSummaryConfig};
use super::openlibrary::{self, OpenLibrarySummaryConfig};
use super::{fetch_summary, fetch_summary_with, summary_source_plan};
use crate::pool::init_db;
use crate::settings::{set_google_books_api_key, set_hardcover_api_key};
use crate::suggestions::hardcover::HardcoverConfig;
use crate::test_support::{seed_synced_ebook, EnvVarGuard};

fn ol_config(server: &MockServer) -> OpenLibrarySummaryConfig {
    OpenLibrarySummaryConfig {
        base_url: server.uri(),
        timeout: std::time::Duration::from_secs(5),
    }
}

fn gb_config(server: &MockServer) -> GoogleBooksSummaryConfig {
    GoogleBooksSummaryConfig {
        base_url: server.uri(),
        api_key: None,
        timeout: std::time::Duration::from_secs(5),
    }
}

/// A Google Books config pointing nowhere useful — for tests that exercise a
/// non-Google branch and never make a Google Books call.
fn unused_googlebooks() -> GoogleBooksSummaryConfig {
    GoogleBooksSummaryConfig::default()
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

// ── Google Books client (wiremock) ───────────────────────────────

#[tokio::test]
async fn googlebooks_fetch_reads_description_for_a_matched_isbn() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .and(query_param("q", "isbn:9780140328721"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "volumeInfo": { "description": "A clever fox outwits three farmers." } }]
        })))
        .mount(&server)
        .await;

    let got = googlebooks::fetch(
        &gb_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        Some("Roald Dahl"),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("A clever fox outwits three farmers."));
}

#[tokio::test]
async fn googlebooks_fetch_falls_back_to_title_author_search_when_no_isbn() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .and(query_param(
            "q",
            "intitle:A Court of Thorns and Roses inauthor:Sarah J. Maas",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "volumeInfo": { "description": "Feyre is dragged into a faerie land." } }]
        })))
        .mount(&server)
        .await;

    let got = googlebooks::fetch(
        &gb_config(&server),
        &[],
        "A Court of Thorns and Roses",
        Some("Sarah J. Maas"),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("Feyre is dragged into a faerie land."));
}

#[tokio::test]
async fn googlebooks_fetch_returns_none_when_no_volume_matches() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .mount(&server)
        .await;

    let got = googlebooks::fetch(
        &gb_config(&server),
        &["9999999999999".to_string()],
        "Nonexistent Book",
        None,
    )
    .await
    .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn googlebooks_fetch_treats_blank_description_as_none() {
    let server = MockServer::start().await;
    // ISBN match with a blank description, and an empty title search → overall miss.
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .and(query_param("q", "isbn:9780140328721"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "volumeInfo": { "description": "   " } }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .and(query_param("q", "intitle:Fantastic Mr Fox"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [] })))
        .mount(&server)
        .await;

    let got = googlebooks::fetch(
        &gb_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        None,
    )
    .await
    .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn googlebooks_fetch_bails_after_retries_on_persistent_503() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .respond_with(ResponseTemplate::new(503))
        .mount(&server)
        .await;

    let err = googlebooks::fetch(
        &gb_config(&server),
        &["9780140328721".to_string()],
        "Fantastic Mr Fox",
        None,
    )
    .await
    .expect_err("a persistent 503 must surface as an error after retries");
    assert!(err.to_string().contains("google books unavailable"));
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
        &unused_googlebooks(),
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
        &unused_googlebooks(),
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
        &unused_googlebooks(),
        &OpenLibrarySummaryConfig::default(),
    )
    .await
    .unwrap();
    assert!(got.is_none());
}

// ── fetch_summary (production entry point) ────────────────────────

#[tokio::test]
async fn fetch_summary_returns_none_immediately_when_no_hardcover_key_is_configured() {
    // No settings row and no `HARDCOVER_API_KEY` env var, so `fetch_summary`
    // must short-circuit before ever resolving the book — an unseeded uuid
    // would fail `get_book_by_uuid` if that lookup ran. Force the env var
    // unset for the duration of the test so a developer's or CI runner's
    // ambient `HARDCOVER_API_KEY` can't mask the short-circuit.
    let _env = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let got = fetch_summary(&pool, "does-not-exist", SummarySource::Hardcover)
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn fetch_summary_dispatches_past_the_key_check_once_a_hardcover_key_is_configured() {
    // With a key configured, `fetch_summary` proceeds into `fetch_summary_with`
    // (built against the live Hardcover/OpenLibrary endpoints) rather than
    // short-circuiting — exercised offline via an unseeded book uuid, which
    // `fetch_summary_with` rejects at the `get_book_by_uuid` lookup before any
    // network call would be made.
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("test-key"))
        .await
        .unwrap();
    let got = fetch_summary(&pool, "does-not-exist", SummarySource::Hardcover)
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn fetch_summary_returns_none_for_an_unknown_book_uuid_via_openlibrary_source() {
    // Same collapsed-null behavior as an unresolved Hardcover book (see
    // `fetch_summary_with_hardcover_returns_none_when_book_unresolved`):
    // an unknown uuid and a source with no summary both resolve to a clean
    // `Ok(None)` a caller cascades past — intentional per
    // `server/src/backend/summary.rs`'s doc comment, asserted here for the
    // OpenLibrary branch of the production entry point.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let got = fetch_summary(&pool, "does-not-exist", SummarySource::OpenLibrary)
        .await
        .unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn fetch_summary_with_googlebooks_resolves_via_title_search_for_a_seeded_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;
    // Seeded book has no ISBN, so the client goes straight to the title search.
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .and(query_param(
            "q",
            "intitle:Test Source inauthor:Source Author",
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "items": [{ "volumeInfo": { "description": "The seeded book blurb." } }]
        })))
        .mount(&server)
        .await;

    let got = fetch_summary_with(
        &pool,
        &uuid,
        SummarySource::GoogleBooks,
        &unused_hardcover(),
        &gb_config(&server),
        &OpenLibrarySummaryConfig::default(),
    )
    .await
    .unwrap();
    assert_eq!(got.as_deref(), Some("The seeded book blurb."));
}

// ── summary_source_plan (cascade policy) ─────────────────────────

/// Remove both provider env keys so the plan reflects only what the test saves.
fn clear_key_env() -> (EnvVarGuard, EnvVarGuard) {
    (
        EnvVarGuard::set("HARDCOVER_API_KEY", None),
        EnvVarGuard::set("GOOGLE_BOOKS_API_KEY", None),
    )
}

#[tokio::test]
async fn summary_source_plan_is_googlebooks_then_openlibrary_when_no_keys() {
    let _env = clear_key_env();
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan = summary_source_plan(&pool).await.unwrap();
    // No keys → Open Library is the keyless fallback after Google Books.
    assert_eq!(
        plan,
        vec![SummarySource::GoogleBooks, SummarySource::OpenLibrary]
    );
}

#[tokio::test]
async fn summary_source_plan_leads_with_hardcover_when_only_its_key_is_set() {
    let _env = clear_key_env();
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("hc_test_key_1234567890"))
        .await
        .unwrap();
    let plan = summary_source_plan(&pool).await.unwrap();
    // A key exists, so Open Library is dropped.
    assert_eq!(
        plan,
        vec![SummarySource::Hardcover, SummarySource::GoogleBooks]
    );
}

#[tokio::test]
async fn summary_source_plan_drops_openlibrary_when_only_googlebooks_key_is_set() {
    let _env = clear_key_env();
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_google_books_api_key(&pool, Some("gb_test_key"))
        .await
        .unwrap();
    let plan = summary_source_plan(&pool).await.unwrap();
    assert_eq!(plan, vec![SummarySource::GoogleBooks]);
}

#[tokio::test]
async fn summary_source_plan_uses_hardcover_and_googlebooks_when_both_keys_are_set() {
    let _env = clear_key_env();
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_hardcover_api_key(&pool, Some("hc_test_key_1234567890"))
        .await
        .unwrap();
    set_google_books_api_key(&pool, Some("gb_test_key"))
        .await
        .unwrap();
    let plan = summary_source_plan(&pool).await.unwrap();
    assert_eq!(
        plan,
        vec![SummarySource::Hardcover, SummarySource::GoogleBooks]
    );
}
