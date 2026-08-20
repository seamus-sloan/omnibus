//! Unit tests for `external_ratings`: the `(book, provider)` upsert, the
//! attributed read projection, the provider fan-out `refresh_ratings` drives,
//! and the two properties a cached provider fact has to keep — a provider with
//! nothing to say writes no row, and a reindex leaves every row alone.

use omnibus_shared::EbookMetadata;
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::metadata_lookup::ProviderKeys;
use crate::{init_db, replace_books};

/// Effective Java's ISBN-13 — the edition every fixture below describes.
const ISBN13: &str = "9780134685991";

const GB_PATH: &str = "/books/v1/volumes";
const OL_SEARCH_PATH: &str = "/search.json";
const OL_RATINGS_PATH: &str = "/works/OL1W/ratings.json";
/// Hardcover's GraphQL endpoint is the base URL itself, so it lands on root.
const HC_PATH: &str = "/";

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> String {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{}.epub", title.to_lowercase()),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .expect("seed book");
    crate::list_books(pool, library)
        .await
        .unwrap()
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .and_then(|b| b.unique_identifier)
        .expect("seeded book should have a uuid")
}

fn rating(score: f64, count: Option<i64>) -> ProviderRating {
    ProviderRating::new(Some(score), 5.0, count, None).expect("fixture score is real")
}

/// A config pointing every provider at `server`, with both optional keys set
/// so all three are `configured` and the fan-out asks each one.
fn all_keyed_config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: server.uri(),
        googlebooks_base: server.uri(),
        hardcover_base: server.uri(),
        keys: ProviderKeys {
            googlebooks: Some("gb-key".into()),
            hardcover: Some("hc-key".into()),
        },
        timeout: std::time::Duration::from_secs(5),
    }
}

/// Every provider answering with a community rating for [`ISBN13`].
async fn mount_rating_bearing(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [{
            "id": "gbvol",
            "volumeInfo": {
                "title": "Effective Java",
                "averageRating": 4.5,
                "ratingsCount": 1_840,
                "infoLink": "https://books.google.com/books?id=gbvol",
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
            }
        }]})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "key": "/works/OL1W" }]
        })))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_RATINGS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "summary": { "average": 3.75, "count": 42 }
        })))
        .mount(server)
        .await;
    Mock::given(method("POST"))
        .and(path(HC_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": {
                "editions": [{ "book_id": 7 }],
                "books": [{ "id": 7, "slug": "effective-java", "rating": 4.1, "ratings_count": 12 }],
            }
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn upsert_rating_stores_the_score_on_the_providers_own_scale() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;

    upsert_rating(
        &pool,
        &uuid,
        MetadataProvider::GoogleBooks,
        &ProviderRating::new(Some(4.5), 5.0, Some(1_840), Some("https://x/y".into())).unwrap(),
    )
    .await
    .unwrap();

    let stored = list_ratings(&pool, &uuid).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].provider, MetadataProvider::GoogleBooks);
    assert_eq!(stored[0].display_name, "Google Books");
    assert_eq!(stored[0].rating, 4.5);
    assert_eq!(stored[0].rating_max, 5.0);
    assert_eq!(stored[0].ratings_count, Some(1_840));
    assert_eq!(stored[0].source_url.as_deref(), Some("https://x/y"));
}

#[tokio::test]
async fn upsert_rating_updates_the_same_providers_row_in_place() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;

    upsert_rating(
        &pool,
        &uuid,
        MetadataProvider::Hardcover,
        &rating(3.0, Some(5)),
    )
    .await
    .unwrap();
    upsert_rating(
        &pool,
        &uuid,
        MetadataProvider::Hardcover,
        &rating(4.0, Some(9)),
    )
    .await
    .unwrap();

    let stored = list_ratings(&pool, &uuid).await.unwrap();
    assert_eq!(stored.len(), 1, "the primary key must collapse the two");
    assert_eq!(stored[0].rating, 4.0);
    assert_eq!(stored[0].ratings_count, Some(9));
}

#[tokio::test]
async fn upsert_rating_keeps_each_providers_row_separate() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;

    for provider in MetadataProvider::ALL {
        upsert_rating(&pool, &uuid, provider, &rating(4.0, None))
            .await
            .unwrap();
    }

    assert_eq!(list_ratings(&pool, &uuid).await.unwrap().len(), 3);
}

#[tokio::test]
async fn upsert_rating_returns_book_not_found_for_an_unindexed_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    assert!(matches!(
        upsert_rating(
            &pool,
            "nope",
            MetadataProvider::Hardcover,
            &rating(4.0, None)
        )
        .await,
        Err(ExternalRatingsError::BookNotFound)
    ));
}

#[tokio::test]
async fn upsert_rating_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;

    assert!(matches!(
        upsert_rating(
            &pool,
            "any",
            MetadataProvider::Hardcover,
            &rating(4.0, None)
        )
        .await,
        Err(ExternalRatingsError::Sqlx(_))
    ));
}

#[tokio::test]
async fn list_ratings_returns_empty_for_an_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    assert!(list_ratings(&pool, "nope").await.unwrap().is_empty());
}

#[tokio::test]
async fn list_ratings_resolves_a_merged_uuid_to_the_surviving_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('old-uuid', ?, 'EPUB', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    upsert_rating(
        &pool,
        &uuid,
        MetadataProvider::OpenLibrary,
        &rating(3.5, None),
    )
    .await
    .unwrap();

    let stored = list_ratings(&pool, "old-uuid").await.unwrap();
    assert_eq!(stored.len(), 1);
}

#[tokio::test]
async fn list_ratings_skips_a_row_naming_a_provider_this_build_does_not_know() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    sqlx::query(
        "INSERT INTO book_external_ratings
             (book_uuid, provider, rating, rating_max, fetched_at)
         VALUES (?, 'goodreads', 4.0, 5.0, 0)",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    assert!(list_ratings(&pool, &uuid).await.unwrap().is_empty());
}

#[tokio::test]
async fn list_ratings_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    pool.close().await;

    assert!(matches!(
        list_ratings(&pool, &uuid).await,
        Err(ExternalRatingsError::Sqlx(_))
    ));
}

#[tokio::test]
async fn refresh_ratings_stores_one_row_per_provider_that_reported_a_score() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let server = MockServer::start().await;
    mount_rating_bearing(&server).await;

    let stored = refresh_ratings(&pool, &all_keyed_config_for(&server), &uuid, ISBN13)
        .await
        .unwrap();

    assert_eq!(stored.len(), 3);
    let by_provider = |p: MetadataProvider| {
        stored
            .iter()
            .find(|r| r.provider == p)
            .unwrap_or_else(|| panic!("{p:?} should have contributed a row"))
    };
    assert_eq!(by_provider(MetadataProvider::GoogleBooks).rating, 4.5);
    assert_eq!(
        by_provider(MetadataProvider::GoogleBooks).ratings_count,
        Some(1_840)
    );
    assert_eq!(by_provider(MetadataProvider::OpenLibrary).rating, 3.75);
    assert_eq!(by_provider(MetadataProvider::Hardcover).rating, 4.1);
    assert_eq!(
        by_provider(MetadataProvider::Hardcover)
            .source_url
            .as_deref(),
        Some("https://hardcover.app/books/effective-java")
    );
}

#[tokio::test]
async fn refresh_ratings_writes_no_row_for_a_provider_with_no_rating() {
    // Absent is not zero: an unrated volume must leave the table untouched
    // rather than render as "0/5".
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [{
            "volumeInfo": {
                "title": "Effective Java",
                "averageRating": 0,
                "ratingsCount": 0,
                "industryIdentifiers": [{ "type": "ISBN_13", "identifier": ISBN13 }],
            }
        }]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [] })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(HC_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "editions": [], "books": [] }
        })))
        .mount(&server)
        .await;

    let stored = refresh_ratings(&pool, &all_keyed_config_for(&server), &uuid, ISBN13)
        .await
        .unwrap();

    assert!(stored.is_empty());
    assert_eq!(
        crate::test_support::count_rows(&pool, "SELECT COUNT(*) FROM book_external_ratings").await,
        0
    );
}

#[tokio::test]
async fn refresh_ratings_keeps_the_other_providers_when_one_fails() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "key": "/works/OL1W" }]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_RATINGS_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "summary": { "average": 3.75, "count": 42 }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path(HC_PATH))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let stored = refresh_ratings(&pool, &all_keyed_config_for(&server), &uuid, ISBN13)
        .await
        .unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].provider, MetadataProvider::OpenLibrary);
}

#[tokio::test]
async fn refresh_ratings_updates_in_place_when_a_candidate_is_applied_twice() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let server = MockServer::start().await;
    mount_rating_bearing(&server).await;
    let config = all_keyed_config_for(&server);

    refresh_ratings(&pool, &config, &uuid, ISBN13)
        .await
        .unwrap();
    let stored = refresh_ratings(&pool, &config, &uuid, ISBN13)
        .await
        .unwrap();

    assert_eq!(stored.len(), 3);
    assert_eq!(
        crate::test_support::count_rows(&pool, "SELECT COUNT(*) FROM book_external_ratings").await,
        3
    );
}

#[tokio::test]
async fn refresh_ratings_returns_book_not_found_for_an_unindexed_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let server = MockServer::start().await;

    assert!(matches!(
        refresh_ratings(&pool, &all_keyed_config_for(&server), "nope", ISBN13).await,
        Err(ExternalRatingsError::BookNotFound)
    ));
}

#[tokio::test]
async fn refresh_ratings_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let server = MockServer::start().await;
    pool.close().await;

    assert!(matches!(
        refresh_ratings(&pool, &all_keyed_config_for(&server), "any", ISBN13).await,
        Err(ExternalRatingsError::Sqlx(_))
    ));
}

#[tokio::test]
async fn refresh_ratings_never_asks_an_unconfigured_provider() {
    // Hardcover is key-gated; without one the fan-out must issue no request,
    // which the un-mounted GraphQL endpoint would otherwise 404 on.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(GB_PATH))
        .and(query_param("q", format!("isbn:{ISBN13}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "items": [{
            "volumeInfo": { "title": "Effective Java", "averageRating": 4.5 }
        }]})))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(OL_SEARCH_PATH))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [] })))
        .mount(&server)
        .await;
    let config = MetadataLookupConfig {
        keys: ProviderKeys::default(),
        ..all_keyed_config_for(&server)
    };

    let stored = refresh_ratings(&pool, &config, &uuid, ISBN13)
        .await
        .unwrap();

    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].provider, MetadataProvider::GoogleBooks);
    assert!(server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .all(|r| r.method != wiremock::http::Method::POST));
}

#[tokio::test]
async fn a_reindex_leaves_the_stored_ratings_intact() {
    // The table soft-references `books.uuid` with no FK and no cascade, so a
    // rescan of the same library must not touch it.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed(&pool, "/lib", "Book A").await;
    upsert_rating(
        &pool,
        &uuid,
        MetadataProvider::OpenLibrary,
        &rating(3.5, Some(4)),
    )
    .await
    .unwrap();

    let after = seed(&pool, "/lib", "Book A").await;

    assert_eq!(after, uuid, "the reindex must preserve the book uuid");
    let stored = list_ratings(&pool, &uuid).await.unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].rating, 3.5);
}
