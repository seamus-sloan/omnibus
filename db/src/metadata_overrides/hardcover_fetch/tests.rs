//! Tests for the "Fetch from Hardcover" metadata-preview query: the
//! provider-injectable orchestrator (against `wiremock`) and the
//! key-configured/not-configured outer wrapper.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use omnibus_shared::metadata_fetch::HardcoverFetchResult;

use super::{fetch_hardcover_metadata, fetch_hardcover_metadata_with};
use crate::pool::init_db;
use crate::suggestions::hardcover::HardcoverConfig;
use crate::test_support::{seed_synced_ebook, EnvVarGuard};

fn config_for(server: &MockServer) -> HardcoverConfig {
    HardcoverConfig {
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        timeout: std::time::Duration::from_secs(5),
    }
}

/// Mount the standard title-resolve → book-detail → edition-ISBN mock chain
/// for a book that resolves cleanly. The seeded test book has no ISBN
/// identifier, so `resolve_book` takes its title-fallback branch.
async fn mount_found_book(server: &MockServer) {
    Mock::given(method("POST"))
        .and(body_string_contains("title: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 714600, "slug": "fourth-wing", "title": "Fourth Wing",
                "contributions": [{ "author": { "name": "Rebecca Yarros" } }],
                "book_series": [{ "position": 1.0, "series": { "name": "The Empyrean" } }],
                "image": { "url": "https://example.com/c.jpg" }
            }] }
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_eq: 714600}}"))
        .and(body_string_contains("description"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 714600, "slug": "fourth-wing", "title": "Fourth Wing",
                "contributions": [{ "author": { "name": "Rebecca Yarros" } }],
                "book_series": [{ "position": 1.0, "series": { "name": "The Empyrean" } }],
                "image": { "url": "https://example.com/c.jpg" },
                "description": "  Violet enters Basgiath War College.  "
            }] }
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("editions(where: {book_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "editions": [{ "isbn_13": "9781649374042" }] }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn fetch_hardcover_metadata_with_returns_found_for_a_resolvable_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "fourth-wing.epub", "Fourth Wing", "Rebecca Yarros").await;
    let server = MockServer::start().await;
    mount_found_book(&server).await;

    let result = fetch_hardcover_metadata_with(&pool, &uuid, &config_for(&server))
        .await
        .unwrap();
    let HardcoverFetchResult::Found(meta) = result else {
        panic!("expected Found, got {result:?}");
    };
    assert_eq!(meta.title.as_deref(), Some("Fourth Wing"));
    assert_eq!(meta.authors, vec!["Rebecca Yarros".to_string()]);
    assert_eq!(
        meta.description.as_deref(),
        Some("Violet enters Basgiath War College.")
    );
    assert_eq!(meta.series.as_deref(), Some("The Empyrean"));
    assert_eq!(meta.series_index.as_deref(), Some("1"));
    assert_eq!(meta.isbn13.as_deref(), Some("9781649374042"));
}

#[tokio::test]
async fn fetch_hardcover_metadata_with_returns_not_found_when_hardcover_has_no_match() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "obscure.epub", "Totally Obscure Book", "Nobody").await;
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("title: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [] }
        })))
        .mount(&server)
        .await;

    let result = fetch_hardcover_metadata_with(&pool, &uuid, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(result, HardcoverFetchResult::NotFound);
}

#[tokio::test]
async fn fetch_hardcover_metadata_with_errors_when_the_book_uuid_is_unknown() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let server = MockServer::start().await;

    let err = fetch_hardcover_metadata_with(&pool, "no-such-uuid", &config_for(&server))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("book not found"));
}

#[tokio::test]
async fn fetch_hardcover_metadata_returns_not_configured_when_no_key_is_set() {
    let _guard = EnvVarGuard::set("HARDCOVER_API_KEY", None);
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "fourth-wing.epub", "Fourth Wing", "Rebecca Yarros").await;

    let result = fetch_hardcover_metadata(&pool, &uuid).await.unwrap();
    assert_eq!(result, HardcoverFetchResult::NotConfigured);
}
