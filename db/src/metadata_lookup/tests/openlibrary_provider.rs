//! Open Library provider tests: the parse-failure path and the best-effort
//! enrichment lookup (series, first-publish year) that fills the fields no
//! other provider carries. Ladder-level ordering lives in the parent module.

use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider, ProviderEdition};
use serde_json::json;
use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers::openlibrary;
use super::super::*;
use super::{
    config_for, gb_hit, mount_gb, mount_ol, mount_ol_search, ol_hit, ol_search_hit, ISBN10, ISBN13,
    OL_PATH, QUERY,
};

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

// ── the picker's fields: genres, print pages, ISBN-10 ────────────

#[tokio::test]
async fn openlibrary_lookup_populates_genres_print_pages_and_isbn10() {
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;

    let edition = openlibrary::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture record must resolve");
    assert_eq!(
        edition.genres,
        vec!["Java (Computer program language)", "Programming"]
    );
    assert_eq!(edition.pages, Some(416));
    assert_eq!(edition.isbn10.as_deref(), Some(ISBN10));
}

#[tokio::test]
async fn openlibrary_caps_a_long_subject_list_and_keeps_its_order() {
    // Open Library's subject lists run to dozens of entries — genre, setting,
    // and character names mixed — and a chip editor handed all of them is
    // unusable. The cap keeps the head, which is the relevant end.
    let server = MockServer::start().await;
    let subjects: Vec<serde_json::Value> = (0..40)
        .map(|i| json!({ "name": format!("Subject {i}") }))
        .collect();
    let mut body = ol_hit();
    body[format!("ISBN:{ISBN13}")]["subjects"] = json!(subjects);
    mount_ol(&server, body).await;

    let edition = openlibrary::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture record must resolve");
    assert_eq!(edition.genres.len(), ProviderEdition::MAX_GENRES);
    assert_eq!(edition.genres[0], "Subject 0");
    assert_eq!(
        edition.genres[ProviderEdition::MAX_GENRES - 1],
        format!("Subject {}", ProviderEdition::MAX_GENRES - 1)
    );
}

#[tokio::test]
async fn openlibrary_leaves_the_new_fields_unset_when_the_record_omits_them() {
    // A record reporting 0 pages is one whose length Open Library doesn't
    // know — `None`, never `Some(0)`.
    let server = MockServer::start().await;
    mount_ol(
        &server,
        json!({ format!("ISBN:{ISBN13}"): {
            "title": "Effective Java",
            "number_of_pages": 0,
        }}),
    )
    .await;

    let edition = openlibrary::by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture record must resolve");
    assert!(edition.genres.is_empty());
    assert_eq!(edition.pages, None);
    assert_eq!(edition.isbn10, None);
}

#[tokio::test]
async fn openlibrary_search_pairs_the_isbn10_with_the_edition_it_answered_with() {
    // `search.json` answers *works*: one `isbn` list spans every edition, so
    // only the entry that re-derives the returned ISBN-13 is safe to report.
    let server = MockServer::start().await;
    mount_ol_search(&server, ol_search_hit()).await;

    let results = openlibrary::by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    let first = results.first().expect("the fixture doc must map");
    assert_eq!(first.isbn13, ISBN13);
    assert_eq!(first.isbn10.as_deref(), Some(ISBN10));
    assert_eq!(
        first.genres,
        vec!["Java (Computer program language)", "Programming"]
    );
}

#[tokio::test]
async fn openlibrary_search_drops_an_isbn10_from_another_edition_of_the_work() {
    let server = MockServer::start().await;
    mount_ol_search(
        &server,
        json!({ "docs": [{
            "title": "Effective Java",
            // The work's other printing only: it pairs with a different
            // ISBN-13 than the one this candidate is answered with.
            "isbn": [ISBN13, "0141439513"],
        }]}),
    )
    .await;

    let results = openlibrary::by_title(&config_for(&server), QUERY)
        .await
        .unwrap();
    let first = results.first().expect("the fixture doc must map");
    assert_eq!(first.isbn13, ISBN13);
    assert_eq!(first.isbn10, None);
}

// ── Community ratings ────────────────────────────────────────────

/// Mount the `isbn:` work-key search every ratings lookup starts with.
async fn mount_ol_work_key(server: &MockServer, key: serde_json::Value) {
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .and(query_param("q", format!("isbn:{ISBN13}")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [{ "key": key }] })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn openlibrary_ratings_reads_the_works_summary_and_links_to_the_work() {
    let server = MockServer::start().await;
    mount_ol_work_key(&server, json!("/works/OL1W")).await;
    Mock::given(method("GET"))
        .and(path("/works/OL1W/ratings.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "summary": { "average": 3.75, "count": 42 } })),
        )
        .mount(&server)
        .await;
    let config = config_for(&server);

    let rating = openlibrary::ratings(&config, ISBN13)
        .await
        .unwrap()
        .expect("a rated work should answer");

    assert_eq!(rating.rating, 3.75);
    assert_eq!(rating.rating_max, 5.0);
    assert_eq!(rating.ratings_count, Some(42));
    assert_eq!(
        rating.source_url,
        Some(format!("{}/works/OL1W", server.uri()))
    );
}

#[tokio::test]
async fn openlibrary_ratings_accepts_the_flattened_search_index_spelling() {
    // `ratings.json` answers `summary.{average,count}`; the search index
    // publishes the same numbers as `ratings_average` / `ratings_count`.
    let server = MockServer::start().await;
    mount_ol_work_key(&server, json!("/works/OL1W")).await;
    Mock::given(method("GET"))
        .and(path("/works/OL1W/ratings.json"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(json!({ "ratings_average": 4.25, "ratings_count": 8 })),
        )
        .mount(&server)
        .await;

    let rating = openlibrary::ratings(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the flattened spelling should parse");

    assert_eq!(rating.rating, 4.25);
    assert_eq!(rating.ratings_count, Some(8));
}

#[tokio::test]
async fn openlibrary_ratings_is_none_when_the_isbn_resolves_to_no_work() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "docs": [] })))
        .mount(&server)
        .await;

    assert!(openlibrary::ratings(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn openlibrary_ratings_degrades_a_provider_failure_to_no_rating() {
    // Best-effort like `enrich`: a hiccup costs a line on the detail page, and
    // must never fail the apply that asked for it.
    let server = MockServer::start().await;
    mount_ol_work_key(&server, json!("/works/OL1W")).await;
    Mock::given(method("GET"))
        .and(path("/works/OL1W/ratings.json"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    assert!(openlibrary::ratings(&config_for(&server), ISBN13)
        .await
        .unwrap()
        .is_none());
}
