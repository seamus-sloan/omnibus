//! Hardcover provider tests. It is key-gated, sits last on the ladder, and is
//! never terminal — so an instance that hasn't configured it behaves exactly
//! as before, and one that has can't have a Hardcover outage turn a clean miss
//! into a user-facing outage.

use omnibus_shared::metadata_lookup::MetadataProvider;
use serde_json::json;
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::providers;
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

/// Substring unique to the `books(where: {id:` query, so a mock can tell it
/// from the edition lookup and the `search` endpoint that share this URL.
/// Held as a constant because an unbalanced brace inside a string literal is
/// exactly the kind of thing a naive parser trips over.
///
/// Its `{title:` sibling went with the exact-title filter this provider used
/// to send; text search now goes through `search` (see `SEARCH_QUERY_MARKER`).
const ID_QUERY_MARKER: &str = "books(where: {id:";

/// A Hardcover `books` row, as both the id lookup and the title search return.
fn hc_book() -> serde_json::Value {
    json!({
        "id": 42,
        "title": "Effective Java",
        "description": "  The definitive guide.  ",
        "contributions": [{ "author": { "name": "Joshua Bloch" } }],
        "book_series": [{ "position": 3, "series": { "name": "The Java Series" } }],
        "image": { "url": "https://hc.test/cover.jpg" },
        "editions": [{ "isbn_13": ISBN13 }],
    })
}

/// Substring unique to the `search` query, so a mock can tell it from the two
/// `books(where:)` queries that share the same URL.
const SEARCH_QUERY_MARKER: &str = "query_type";

/// One `search` hit document. A different shape from [`hc_book`] on purpose:
/// the search endpoint answers with `author_names` / `featured_series` /
/// `genres` where the `books` query answers with `contributions` /
/// `book_series` / `cached_tags`, and its `id` is a **string**.
fn hc_search_document() -> serde_json::Value {
    json!({
        "id": "42",
        "slug": "effective-java",
        "title": "Effective Java",
        "description": "  The definitive guide.  ",
        "author_names": ["Joshua Bloch"],
        "featured_series": { "position": 3.0, "series": { "name": "The Java Series" } },
        "genres": ["Programming", "Reference"],
        "pages": 416,
        "release_year": 2018,
        "image": { "url": "https://hc.test/cover.jpg" },
        // Spans every edition of the work, which is exactly why no single one
        // of them can be attributed to this candidate.
        "isbns": [ISBN13, ISBN10, "9780141439518"],
    })
}

/// Mount the `search` endpoint with the given hit documents.
async fn mount_hc_search(server: &MockServer, documents: serde_json::Value) {
    Mock::given(method("POST"))
        .and(wiremock::matchers::body_string_contains(
            SEARCH_QUERY_MARKER.to_string(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "search": { "results": {
                "found": 1,
                "hits": documents,
            }}},
        })))
        .mount(server)
        .await;
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
async fn hardcover_text_search_maps_a_search_document() {
    let server = MockServer::start().await;
    mount_hc_search(&server, json!([{ "document": hc_search_document() }])).await;

    let results = hardcover::by_text(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), 1);
    let first = &results[0];
    assert_eq!(first.source, MetadataProvider::Hardcover);
    assert_eq!(first.title, "Effective Java");
    assert_eq!(first.authors, vec!["Joshua Bloch"]);
    assert_eq!(first.description.as_deref(), Some("The definitive guide."));
    assert_eq!(first.series.as_deref(), Some("The Java Series"));
    assert_eq!(first.series_index.as_deref(), Some("3"));
    assert_eq!(first.pages, Some(416));
    assert_eq!(first.first_publish_year, Some(2018));
    assert_eq!(first.genres, vec!["Programming", "Reference"]);
    // The book id, which is what `by_ref` re-fetches by.
    assert_eq!(first.provider_ref, "42");
}

#[tokio::test]
async fn hardcover_text_search_never_attributes_a_works_isbn_to_a_candidate() {
    // `isbns` lists every edition Hardcover knows for the work, so no single
    // entry names *this* candidate's printing. Taking one anyway would put a
    // specific edition's identifier on a row describing all of them.
    let server = MockServer::start().await;
    mount_hc_search(&server, json!([{ "document": hc_search_document() }])).await;

    let results = hardcover::by_text(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results[0].isbn13, None);
    assert_eq!(results[0].isbn10, None);
}

#[tokio::test]
async fn hardcover_text_search_skips_a_document_with_no_title_or_id() {
    let server = MockServer::start().await;
    let mut untitled = hc_search_document();
    untitled["title"] = json!("   ");
    let mut unidentified = hc_search_document();
    unidentified["id"] = serde_json::Value::Null;
    mount_hc_search(
        &server,
        json!([{ "document": untitled }, { "document": unidentified }, { "document": null }]),
    )
    .await;

    let results = hardcover::by_text(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert!(results.is_empty(), "got {results:?}");
}

#[tokio::test]
async fn hardcover_text_search_answers_a_phrase_carrying_the_author() {
    // The regression this endpoint switch exists for. The old exact-title
    // filter (`books(where: {title: {_eq: …}})`) matched nothing whatsoever
    // for a query shaped "title author", which is what the picker seeds — so
    // Hardcover reported a clean miss for every book that has an author.
    let server = MockServer::start().await;
    mount_hc_search(&server, json!([{ "document": hc_search_document() }])).await;

    let results = hardcover::by_text(
        &hardcover_config_for(&server),
        "Effective Java Joshua Bloch",
    )
    .await
    .unwrap();
    assert_eq!(results.len(), 1, "a phrase must reach the search endpoint");
}

// ── the picker's fields: genres, print pages, ISBN-10 ────────────

/// [`hc_book`] plus the picker's fields — the `Genre` bucket of `cached_tags`
/// and the representative edition's `isbn_10` / `pages`.
fn hc_book_with_picker_fields() -> serde_json::Value {
    let mut book = hc_book();
    book["cached_tags"] = json!({
        "Genre": [{ "tag": "Programming", "count": 900 }, { "tag": "Reference", "count": 12 }],
        "Mood": [{ "tag": "informative" }],
    });
    book["editions"] = json!([{ "isbn_13": ISBN13, "isbn_10": ISBN10, "pages": 416 }]);
    book
}

#[tokio::test]
async fn hardcover_by_ref_populates_genres_print_pages_and_isbn10() {
    // The `books` shape, reached by handle — which is where the picker's
    // fields come from now that the search document carries no edition.
    let server = MockServer::start().await;
    mount_hc_books(
        &server,
        ID_QUERY_MARKER,
        json!([hc_book_with_picker_fields()]),
    )
    .await;

    let found = hardcover::by_ref(&hardcover_config_for(&server), "42")
        .await
        .unwrap()
        .expect("the fixture row must map");
    // Only the `Genre` bucket of `cached_tags` — moods aren't genres.
    assert_eq!(found.genres, vec!["Programming", "Reference"]);
    assert_eq!(found.pages, Some(416));
    assert_eq!(found.isbn10.as_deref(), Some(ISBN10));
    assert_eq!(found.isbn13.as_deref(), Some(ISBN13));
}

#[tokio::test]
async fn hardcover_leaves_the_new_fields_unset_when_the_row_omits_them() {
    let server = MockServer::start().await;
    mount_hc_books(&server, ID_QUERY_MARKER, json!([hc_book()])).await;

    let found = hardcover::by_ref(&hardcover_config_for(&server), "42")
        .await
        .unwrap()
        .expect("the fixture row must map");
    assert!(found.genres.is_empty());
    assert_eq!(found.pages, None);
    assert_eq!(found.isbn10, None);
}

#[tokio::test]
async fn hardcover_by_ref_refuses_a_handle_that_is_not_a_book_id() {
    // An `isbn:` fallback ref belongs on the ISBN path; addressing it here
    // would interpolate a non-numeric value into an `Int!` variable.
    let server = MockServer::start().await;
    let found = hardcover::by_ref(&hardcover_config_for(&server), "isbn:9780134685991")
        .await
        .unwrap();
    assert!(found.is_none());
}

#[tokio::test]
async fn hardcover_isbn_lookup_drops_edition_fields_from_a_different_printing() {
    // Hardcover resolves to a *work* and hands back its most-read edition,
    // which is often not the printing that was scanned — so that edition's
    // page count and ISBN-10 describe a different book.
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    let mut other_printing = hc_book_with_picker_fields();
    other_printing["editions"] =
        json!([{ "isbn_13": "9780141439518", "isbn_10": "0141439513", "pages": 480 }]);
    mount_hc_books(&server, ID_QUERY_MARKER, json!([other_printing])).await;

    let edition = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("hardcover should resolve the scanned isbn");
    assert_eq!(edition.isbn13.as_deref(), Some(ISBN13));
    assert_eq!(edition.pages, None);
    assert_eq!(edition.isbn10, None);
    // Work-level fields are unaffected — they were never edition-scoped.
    assert_eq!(edition.genres, vec!["Programming", "Reference"]);
}

// ── Series position (the field the retired Hardcover panel could apply) ──

#[tokio::test]
async fn hardcover_carries_the_series_position_as_a_book_number() {
    // The one provider that models a position. Without it the picker cannot
    // offer "Book #", which the panel this replaced could.
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    mount_hc_books(&server, "books(where", json!([hc_book()])).await;

    let found = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture resolves");
    assert_eq!(found.series.as_deref(), Some("The Java Series"));
    assert_eq!(found.series_index.as_deref(), Some("3"));
}

#[tokio::test]
async fn hardcover_formats_a_half_position_for_a_novella() {
    // Positions are floats because a novella sits at 2.5; a bare `{v}` would
    // render the whole numbers as "3" and this as "2.5", so both go through
    // the shared formatter.
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    let mut book = hc_book();
    book["book_series"] = json!([{ "position": 2.5, "series": { "name": "The Java Series" } }]);
    mount_hc_books(&server, "books(where", json!([book])).await;

    let found = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture resolves");
    assert_eq!(found.series_index.as_deref(), Some("2.5"));
}

#[tokio::test]
async fn hardcover_takes_the_name_and_the_position_from_one_series_row() {
    // A book can sit in more than one series. Reading the name off whichever
    // row has one and the position off whichever row has that would file the
    // book under one series at another's number.
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    let mut book = hc_book();
    book["book_series"] = json!([
        { "position": 7, "series": serde_json::Value::Null },
        { "position": 1, "series": { "name": "The Java Series" } },
    ]);
    mount_hc_books(&server, "books(where", json!([book])).await;

    let found = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture resolves");
    assert_eq!(found.series.as_deref(), Some("The Java Series"));
    // 1, from the row that named the series — not 7 from the unnamed one.
    assert_eq!(found.series_index.as_deref(), Some("1"));
}

#[tokio::test]
async fn hardcover_reports_no_book_number_when_the_series_has_no_position() {
    let server = MockServer::start().await;
    mount_hc_edition(&server, Some(42)).await;
    let mut book = hc_book();
    book["book_series"] = json!([{ "series": { "name": "The Java Series" } }]);
    mount_hc_books(&server, "books(where", json!([book])).await;

    let found = hardcover::by_isbn(&hardcover_config_for(&server), ISBN13)
        .await
        .unwrap()
        .expect("the fixture resolves");
    assert_eq!(found.series.as_deref(), Some("The Java Series"));
    assert_eq!(found.series_index, None);
}

#[tokio::test]
async fn hardcover_check_in_rung_resolves_an_edition_for_a_work_level_hit() {
    // The ladder needs an ISBN — check-in stores one per physical copy — and
    // Hardcover's full-text search answers with works. Without the resolve,
    // this rung would answer and then be discarded wholesale.
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, json!({ "totalItems": 0 })).await;
    mount_hc_search(&server, json!([{ "document": hc_search_document() }])).await;
    mount_hc_books(&server, ID_QUERY_MARKER, json!([hc_book()])).await;

    let results = search_provider_by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert_eq!(results.len(), 1, "the rung must still answer");
    assert_eq!(results[0].source, MetadataProvider::Hardcover);
    assert_eq!(
        results[0].isbn13, ISBN13,
        "resolved from the book record, not borrowed from the work's isbns list"
    );
}

#[tokio::test]
async fn hardcover_check_in_rung_is_a_clean_miss_when_the_edition_cannot_be_resolved() {
    // The resolve is a recovery from a rung that already had nothing usable,
    // so its failure is that same miss — never a user-facing provider error.
    let server = MockServer::start().await;
    mount_ol_search(&server, json!({ "docs": [] })).await;
    mount_gb_search(&server, json!({ "totalItems": 0 })).await;
    mount_hc_search(&server, json!([{ "document": hc_search_document() }])).await;
    mount_hc_books(&server, ID_QUERY_MARKER, json!([])).await;

    let results = search_provider_by_title(&hardcover_config_for(&server), QUERY)
        .await
        .unwrap();
    assert!(results.is_empty(), "got {results:?}");
}

#[tokio::test]
async fn hardcover_429_records_a_cooldown() {
    // Hardcover surfaces its transport failure through `HardcoverError`, whose
    // `#[error(transparent)]` forwards `source()` past the `reqwest::Error` —
    // so the shared error-chain sniff cannot see the status and this provider
    // has to record its own.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let config = hardcover_config_for(&server);

    let err = hardcover::by_text(&config, QUERY).await;
    assert!(err.is_err(), "a 429 is a failure, not a miss");
    assert!(
        config
            .throttle
            .remaining(MetadataProvider::Hardcover)
            .is_some(),
        "the refusal must be remembered, or the next search walks back into it"
    );
}

#[tokio::test]
async fn open_library_429_records_a_cooldown() {
    // Open Library wraps with `.context(...)`, which keeps the `reqwest::Error`
    // in the chain — so the shared sniff does see it.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(wiremock::matchers::path("/search.json"))
        .respond_with(ResponseTemplate::new(429))
        .mount(&server)
        .await;
    let config = config_for(&server);

    let _ = providers::run(
        MetadataProvider::OpenLibrary,
        &config,
        &super::title_query(QUERY),
    )
    .await;
    assert!(config
        .throttle
        .remaining(MetadataProvider::OpenLibrary)
        .is_some());
}

#[tokio::test]
async fn hardcover_text_search_reads_a_null_search_envelope_as_no_hits() {
    // `search` is a nullable GraphQL field: Hasura answers `{"data":{"search":
    // null}}` alongside an `errors` array. Failing to decode that would report
    // an outage where the honest answer is "nothing found".
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "search": null } })),
        )
        .mount(&server)
        .await;

    let results = hardcover::by_text(&hardcover_config_for(&server), QUERY)
        .await
        .expect("a null envelope is not a failure");
    assert!(results.is_empty());
}

#[tokio::test]
async fn hardcover_by_ref_refuses_a_handle_wider_than_a_graphql_int() {
    // `$id: Int!` is 32-bit; a wider value would pass a local parse and then
    // be refused server-side as a coercion error.
    let server = MockServer::start().await;
    let found = hardcover::by_ref(&hardcover_config_for(&server), "9999999999")
        .await
        .unwrap();
    assert!(found.is_none());
}
