//! The catalog's capability flags checked against what the parsers actually
//! do, both directions: a flag that outruns its parser renders a permanently
//! blank column in the picker, and a parser that outruns its flag has its
//! column hidden. Asserted per provider against fixtures that carry every
//! field — genres off one fan-out, ratings off the ISBN-keyed lookup they are
//! fetched by.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers::{catalog, ratings as provider_ratings};
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

/// Every provider answering with a community rating, so the ratings flag can
/// be held against a real parse the same way the genre one is. Ratings are a
/// per-book fact rather than an edition field, so they come off the
/// ISBN-keyed lookup, not the search.
async fn mount_rating_bearing(server: &MockServer) {
    let bodies = [
        (
            "GET",
            OL_SEARCH_PATH,
            json!({ "docs": [{ "key": "/works/OL1W" }] }),
        ),
        (
            "GET",
            "/works/OL1W/ratings.json",
            json!({ "summary": { "average": 3.75, "count": 42 } }),
        ),
        (
            "GET",
            GB_PATH,
            json!({ "items": [{ "volumeInfo": { "averageRating": 4.5, "ratingsCount": 1840 } }] }),
        ),
        (
            "POST",
            HC_PATH,
            json!({ "data": {
                "editions": [{ "book_id": 7 }],
                "books": [{ "id": 7, "rating": 4.1, "ratings_count": 12 }],
            }}),
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
async fn every_provider_carrying_ratings_actually_returns_one() {
    let server = MockServer::start().await;
    mount_rating_bearing(&server).await;
    let config = all_keyed_config_for(&server);

    for info in catalog(&config) {
        let returned = provider_ratings(info.id, &config, ISBN13)
            .await
            .unwrap_or_else(|e| panic!("{:?} rating lookup failed: {e:#}", info.id))
            .is_some();
        assert_eq!(
            info.capabilities.carries_ratings, returned,
            "{:?} advertises carries_ratings={} but returned a rating={returned}",
            info.id, info.capabilities.carries_ratings,
        );
    }
}

#[tokio::test]
async fn a_provider_with_no_rating_for_a_book_still_advertises_the_capability() {
    // Same rule as genres: the flag describes the provider's API, not one
    // book. An unrated volume must not read as "this source can't rate".
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(
                json!({ "items": [{ "volumeInfo": { "title": "Effective Java" } }] }),
            ),
        )
        .mount(&server)
        .await;
    let config = all_keyed_config_for(&server);

    assert!(
        provider_ratings(MetadataProvider::GoogleBooks, &config, ISBN13)
            .await
            .unwrap()
            .is_none()
    );
    let info = catalog(&config)
        .into_iter()
        .find(|p| p.id == MetadataProvider::GoogleBooks)
        .expect("catalog should list Google Books");
    assert!(info.capabilities.carries_ratings);
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
