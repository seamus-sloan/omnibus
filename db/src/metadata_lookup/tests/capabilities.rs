//! The catalog's capability flags checked against what the parsers actually
//! do, both directions: a flag that outruns its parser renders a permanently
//! blank column in the picker, and a parser that outruns its flag has its
//! column hidden. Asserted per provider, off one fan-out against fixtures
//! that carry every field.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers::catalog;
use super::super::*;
use super::{all_keyed_config_for, GB_PATH, HC_PATH, ISBN13, OL_SEARCH_PATH, QUERY};

/// Every provider answering with a genre list, so "the flag says yes" can be
/// held against "and here it is".
async fn mount_genre_bearing(server: &MockServer) {
    let bodies = [
        (
            "GET",
            OL_SEARCH_PATH,
            json!({ "docs": [{
                "title": "Effective Java",
                "isbn": [ISBN13],
                "subject": ["Programming"],
            }]}),
        ),
        (
            "GET",
            GB_PATH,
            json!({ "items": [{ "volumeInfo": {
                "title": "Effective Java",
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
                "categories": ["Computers"],
            }}]}),
        ),
        (
            "POST",
            HC_PATH,
            json!({ "data": { "books": [{
                "id": 42,
                "title": "Effective Java",
                "cached_tags": { "Genre": [{ "tag": "Programming" }] },
                "editions": [{ "isbn_13": ISBN13 }],
            }]}}),
        ),
    ];
    for (verb, at, body) in bodies {
        Mock::given(method(verb))
            .and(path(at))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }
}

#[tokio::test]
async fn every_provider_carrying_genres_actually_returns_them() {
    let server = MockServer::start().await;
    mount_genre_bearing(&server).await;
    let config = all_keyed_config_for(&server);

    let response = search_all_providers(&config, QUERY, None).await;
    for info in catalog(&config) {
        let returned_genres = response
            .editions
            .iter()
            .filter(|e| e.source == info.id)
            .any(|e| !e.genres.is_empty());
        assert_eq!(
            info.capabilities.carries_genres, returned_genres,
            "{:?} advertises carries_genres={} but returned genres={returned_genres}",
            info.id, info.capabilities.carries_genres,
        );
    }
}

#[tokio::test]
async fn no_provider_advertises_ratings_it_cannot_answer_with() {
    // Community ratings are a separate, per-provider cached fact rather than
    // an edition field, so nothing parses them yet — the flag must stay false
    // until something does.
    let server = MockServer::start().await;
    for info in catalog(&all_keyed_config_for(&server)) {
        assert!(
            !info.capabilities.carries_ratings,
            "{:?} advertises ratings no parser fills",
            info.id
        );
    }
}

#[tokio::test]
async fn a_provider_with_no_genres_for_a_book_still_advertises_the_capability() {
    // The flag describes the provider's API, not one book: a search that
    // happens to return no genres must not read as "this source can't".
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "docs": [{ "title": "Effective Java", "isbn": [ISBN13] }]})),
        )
        .mount(&server)
        .await;
    let config = all_keyed_config_for(&server);

    let response =
        search_all_providers(&config, QUERY, Some(&[MetadataProvider::OpenLibrary])).await;
    assert!(response.editions.iter().all(|e| e.genres.is_empty()));
    let info = catalog(&config)
        .into_iter()
        .find(|p| p.id == MetadataProvider::OpenLibrary)
        .expect("catalog should list Open Library");
    assert!(info.capabilities.carries_genres);
}

// ── Cover hosts ──────────────────────────────────────────────────

#[test]
fn every_provider_in_the_catalog_declares_at_least_one_cover_host() {
    // A provider whose covers the picker renders but the CSP doesn't name is
    // a cover that silently fails to load, with the only symptom in a browser
    // console. Every catalog entry advertising `carries_cover` must be here.
    let config = MetadataLookupConfig::live(ProviderKeys::default());
    for info in catalog(&config) {
        if info.capabilities.carries_cover {
            assert!(
                !cover_hosts(info.id).is_empty(),
                "{:?} carries covers but declares no host",
                info.id
            );
        }
    }
}

#[test]
fn all_cover_hosts_is_the_deduplicated_union_of_every_providers_hosts() {
    let all = all_cover_hosts();
    let mut sorted = all.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), all.len(), "no host may appear twice: {all:?}");

    for provider in [
        MetadataProvider::OpenLibrary,
        MetadataProvider::GoogleBooks,
        MetadataProvider::Hardcover,
    ] {
        for host in cover_hosts(provider) {
            assert!(all.contains(host), "{host} missing from the union");
        }
    }
}

#[test]
fn open_librarys_cover_hosts_include_the_redirect_target_it_serves_bytes_from() {
    // `covers.openlibrary.org` 302s to `archive.org`, and both the browser's
    // `img-src` check and a server-side fetch follow that hop — so an
    // origin-only list blocks every Open Library cover.
    let hosts = cover_hosts(MetadataProvider::OpenLibrary);
    assert!(hosts.contains(&"covers.openlibrary.org"));
    assert!(hosts.contains(&"archive.org"));
    // And the second hop, to a rotating node name only a wildcard can cover.
    assert!(hosts.contains(&"*.archive.org"));
}
