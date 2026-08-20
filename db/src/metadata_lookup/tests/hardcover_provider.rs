//! Hardcover provider tests. It is key-gated, sits last on the ladder, and is
//! never terminal — so an instance that hasn't configured it behaves exactly
//! as before, and one that has can't have a Hardcover outage turn a clean miss
//! into a user-facing outage.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers::hardcover;
use super::super::*;
use super::{
    config_for, mount_gb, mount_gb_search, mount_ol, mount_ol_search, ol_hit, ISBN10, ISBN13, QUERY,
};

// ── Hardcover rung ───────────────────────────────────────────────
//
// Hardcover is key-gated, sits last on the ladder, and is never terminal —
// so an instance that hasn't configured it behaves exactly as before, and one
// that has can't have a Hardcover outage turn a clean miss into an outage.

/// A config with every provider pointed at the mock server and a Hardcover
/// key configured, so the Hardcover rung is on the ladder.
fn hardcover_config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        keys: ProviderKeys {
            googlebooks: None,
            hardcover: Some("hc-key".into()),
        },
        ..config_for(server)
    }
}

/// Mount the Hardcover edition→book_id query (the first of `by_isbn`'s two).
/// Matched on `isbn_10`, which the book query's nested `editions` selection
/// does not carry — plain `editions(where:` appears in both.
async fn mount_hc_edition(server: &MockServer, book_id: Option<i64>) {
    let body = match book_id {
        Some(id) => json!({ "data": { "editions": [{ "book_id": id }] } }),
        None => json!({ "data": { "editions": [] } }),
    };
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_string_contains("isbn_10:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(server)
        .await;
}

/// Substrings unique to each of the two `books` queries, so a mock can answer
/// them differently. Held as constants because an unbalanced brace inside a
/// string literal is exactly the kind of thing a naive parser trips over.
const ID_QUERY_MARKER: &str = "books(where: {id:";
const TITLE_QUERY_MARKER: &str = "books(where: {title:";

/// A Hardcover `books` row, as both the id lookup and the title search return.
fn hc_book() -> serde_json::Value {
    json!({
        "id": 42,
        "title": "Effective Java",
        "description": "  The definitive guide.  ",
        "contributions": [{ "author": { "name": "Joshua Bloch" } }],
        "book_series": [{ "series": { "name": "The Java Series" } }],
        "image": { "url": "https://hc.test/cover.jpg" },
        "editions": [{ "isbn_13": ISBN13 }],
    })
}

async fn mount_hc_books(server: &MockServer, marker: &str, books: serde_json::Value) {
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_string_contains(marker.to_string()))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": books },
        })))
        .mount(server)
        .await;
}

#[tokio::test]
async fn hardcover_rung_answers_when_both_catalogs_miss() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;
    mount_hc_edition(&server, Some(42)).await;
    mount_hc_books(&server, ID_QUERY_MARKER, json!([hc_book()])).await;

    let meta = search_provider_by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("hardcover should answer what the catalogs could not");
    assert_eq!(meta.source, MetadataProvider::Hardcover);
    assert_eq!(meta.title, "Effective Java");
    assert_eq!(meta.authors, vec!["Joshua Bloch".to_string()]);
    // The scanned barcode stays authoritative — Hardcover resolves to a work,
    // whose representative edition is often a different printing.
    assert_eq!(meta.isbn13, ISBN13);
    // The reason this rung is worth having: a series statement, natively.
    assert_eq!(meta.series.as_deref(), Some("The Java Series"));
    assert_eq!(meta.description.as_deref(), Some("The definitive guide."));
    assert_eq!(meta.cover_url.as_deref(), Some("https://hc.test/cover.jpg"));
}

#[tokio::test]
async fn hardcover_rung_is_skipped_entirely_without_a_key() {
    // No key means no rung: the POST endpoint must never be touched, so an
    // unconfigured instance behaves exactly as it did before Hardcover existed.
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let meta = search_provider_by_isbn(&config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(meta.is_none());
    server.verify().await;
}

#[tokio::test]
async fn hardcover_rung_never_runs_when_a_catalog_already_hit() {
    // It sits last precisely so the common case pays nothing for it.
    let server = MockServer::start().await;
    mount_ol(&server, ol_hit()).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let meta = search_provider_by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(meta.source, MetadataProvider::OpenLibrary);
    server.verify().await;
}

#[tokio::test]
async fn hardcover_failure_never_surfaces_as_a_provider_outage() {
    // Hardcover is non-terminal: the catalogs answered (with a clean miss), so
    // the honest answer is "not found", not "try again later".
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let meta = search_provider_by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .expect("a hardcover outage must not fail the lookup");
    assert!(meta.is_none());
}

#[tokio::test]
async fn hardcover_isbn_miss_falls_through_to_a_clean_unresolved() {
    let server = MockServer::start().await;
    mount_ol(&server, json!({})).await;
    mount_gb(&server, json!({ "totalItems": 0 })).await;
    mount_hc_edition(&server, None).await;

    let meta = search_provider_by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap();
    assert!(meta.is_none(), "an unknown ISBN is a miss, not an error");
}

#[tokio::test]
async fn hardcover_title_search_maps_rows_and_takes_the_isbn_from_the_edition() {
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, json!({ "totalItems": 0 })).await;
    mount_hc_books(&server, TITLE_QUERY_MARKER, json!([hc_book()])).await;

    let results = search_provider_by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].source, MetadataProvider::Hardcover);
    // No caller-supplied ISBN on the search path, so it comes from the row's
    // representative edition.
    assert_eq!(results[0].isbn13, ISBN13);
    assert_eq!(results[0].series.as_deref(), Some("The Java Series"));
}

#[tokio::test]
async fn hardcover_title_search_skips_a_row_with_no_isbn() {
    // Without an ISBN a candidate can't be checked in, so it isn't offered.
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, json!({ "totalItems": 0 })).await;
    let mut isbnless = hc_book();
    isbnless["editions"] = json!([]);
    mount_hc_books(&server, TITLE_QUERY_MARKER, json!([isbnless])).await;

    let results = search_provider_by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert!(results.is_empty());
}

// ── the picker's fields: genres, print pages, ISBN-10 ────────────

/// The enriched fields Hardcover only answers with when the selection set
/// asks for them, layered onto [`hc_book`].
fn hc_book_enriched() -> serde_json::Value {
    let mut book = hc_book();
    book["cached_tags"] = json!({
        "Genre": [{ "tag": "Programming", "count": 900 }, { "tag": "Reference", "count": 12 }],
        "Mood": [{ "tag": "informative" }],
    });
    book["editions"] = json!([{ "isbn_13": ISBN13, "isbn_10": ISBN10, "pages": 416 }]);
    book
}

#[tokio::test]
async fn hardcover_title_search_populates_genres_print_pages_and_isbn10() {
    let server = MockServer::start().await;
    mount_hc_books(&server, TITLE_QUERY_MARKER, json!([hc_book_enriched()])).await;

    let results = hardcover::by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    let first = results.first().expect("the fixture row must map");
    // Only the `Genre` bucket of `cached_tags` — moods aren't genres.
    assert_eq!(first.genres, vec!["Programming", "Reference"]);
    assert_eq!(first.pages, Some(416));
    assert_eq!(first.isbn10.as_deref(), Some(ISBN10));
}

#[tokio::test]
async fn hardcover_leaves_the_new_fields_unset_when_the_row_omits_them() {
    let server = MockServer::start().await;
    mount_hc_books(&server, TITLE_QUERY_MARKER, json!([hc_book()])).await;

    let results = hardcover::by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    let first = results.first().expect("the fixture row must map");
    assert!(first.genres.is_empty());
    assert_eq!(first.pages, None);
    assert_eq!(first.isbn10, None);
}

#[tokio::test]
async fn hardcover_isbn_lookup_drops_edition_fields_from_a_different_printing() {
    // Hardcover resolves to a *work* and hands back its most-read edition,
    // which is often not the printing that was scanned — so that edition's
    // page count and ISBN-10 describe a different book.
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    let mut other_printing = hc_book_enriched();
    other_printing["editions"] =
        json!([{ "isbn_13": "9780141439518", "isbn_10": "0141439513", "pages": 480 }]);
    mount_hc_books(&server, ID_QUERY_MARKER, json!([other_printing])).await;

    let edition = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("hardcover should resolve the scanned isbn");
    assert_eq!(edition.isbn13, ISBN13);
    assert_eq!(edition.pages, None);
    assert_eq!(edition.isbn10, None);
    // Work-level fields are unaffected — they were never edition-scoped.
    assert_eq!(edition.genres, vec!["Programming", "Reference"]);
}

#[tokio::test]
async fn hardcover_degrades_to_the_base_selection_when_the_enriched_one_is_rejected() {
    // Hasura answers an unknown field with a 200 carrying `errors[]`. A schema
    // drift must cost the three new fields, not the whole provider — without
    // the fallback every Hardcover lookup, check-in's included, would fail.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_string_contains("cached_tags"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "field 'cached_tags' not found in type: 'books'" }],
        })))
        .mount(&server)
        .await;
    mount_hc_books(&server, TITLE_QUERY_MARKER, json!([hc_book()])).await;

    let results = hardcover::by_title(&hardcover_config_for(&server), QUERY)
        .await
        .expect("a rejected selection must degrade, not fail the provider");
    let first = results.first().expect("the retry must still map the row");
    assert_eq!(first.title, "Effective Java");
    assert_eq!(first.isbn13, ISBN13);
    assert!(first.genres.is_empty());
    assert_eq!(first.pages, None);
}
