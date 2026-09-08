//! The Hardcover GraphQL client against `wiremock`: `resolve_book` by
//! ISBN (priority order, an earlier error surfaced) or title, curated
//! list ids, the error envelope, co-listed counts, `fetch_candidates`'s
//! rank order, `book_description`, and the HTTP-failure paths.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::config_for;
use crate::suggestions::hardcover::{
    book_description, co_listed_counts, curated_list_ids, fetch_candidates, resolve_book,
    HardcoverError,
};

#[tokio::test]
async fn resolve_book_prefers_isbn_match() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("editions(where:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "editions": [{ "book_id": 714600 }] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 714600, "slug": "fourth-wing", "title": "Fourth Wing",
                "contributions": [{ "author": { "name": "Rebecca Yarros" } }],
                "book_series": [{ "position": 1, "series": { "name": "The Empyrean" } }],
                "image": { "url": "https://example.com/c.jpg" }
            }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let resolved = resolve_book(
        &cfg,
        &["9781649374042".to_string()],
        "Fourth Wing",
        Some("Rebecca Yarros"),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(resolved.id, 714600);
    assert_eq!(resolved.author_names, vec!["Rebecca Yarros".to_string()]);
    assert_eq!(resolved.series.as_deref(), Some("The Empyrean"));
}

#[tokio::test]
async fn resolve_book_falls_back_to_title_when_no_isbn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("title: {_eq:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 99, "slug": "x", "title": "Some Title",
                "contributions": [{ "author": { "name": "Jane Doe" } }],
                "book_series": [], "image": null
            }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let resolved = resolve_book(&cfg, &[], "Some Title", Some("Jane Doe"))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resolved.id, 99);
    assert!(resolved.series.is_none());
}

#[tokio::test]
async fn resolve_book_prefers_the_first_isbn_in_priority_order_when_multiple_match() {
    // `resolve_book` now looks up every ISBN with bounded concurrency instead
    // of a sequential loop, so both lookups fire in parallel — but the first
    // ISBN in the caller's list must still win, exactly as the old
    // short-circuiting `for` loop would have picked it.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("9781111111111"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "editions": [{ "book_id": 111 }] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("9782222222222"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "editions": [{ "book_id": 222 }] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_eq: 111}}"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 111, "slug": "first", "title": "First",
                "contributions": [], "book_series": [], "image": null
            }] }
        })))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_eq: 222}}"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 222, "slug": "second", "title": "Second",
                "contributions": [], "book_series": [], "image": null
            }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let resolved = resolve_book(
        &cfg,
        &["9781111111111".to_string(), "9782222222222".to_string()],
        "Irrelevant Title",
        None,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        resolved.id, 111,
        "first ISBN's match must win, not id order"
    );
}

#[tokio::test]
async fn curated_list_ids_returns_list_ids() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {book_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": [{ "list_id": 1 }, { "list_id": 2 }, { "list_id": 3 }] }
        })))
        .mount(&server)
        .await;
    let cfg = config_for(&server);
    let ids = curated_list_ids(&cfg, 714600).await.unwrap();
    assert_eq!(ids, vec![1, 2, 3]);
}

#[tokio::test]
async fn post_graphql_surfaces_graphql_error_envelope() {
    // Hasura returns HTTP 200 even for a failed operation, carrying the
    // failure in an `errors[]` array. `post_graphql` must join those messages
    // into `HardcoverError::Graphql` rather than treating the 200 as success
    // or trying to decode the (absent) `data`. Driven through `curated_list_ids`
    // since `post_graphql` is private.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "field 'list_books' not found" }]
        })))
        .mount(&server)
        .await;
    let cfg = config_for(&server);
    let err = curated_list_ids(&cfg, 714600)
        .await
        .expect_err("an errors[] envelope must not decode as success");
    assert!(
        matches!(&err, crate::suggestions::hardcover::HardcoverError::Graphql(msg)
            if msg.contains("field 'list_books' not found")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn co_listed_counts_ranks_by_shared_list_appearances() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {list_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": [
                { "book_id": 200 }, { "book_id": 200 }, { "book_id": 200 },
                { "book_id": 201 }, { "book_id": 201 },
                { "book_id": 714600 }, // the source book — excluded
                { "book_id": 300 }
            ] }
        })))
        .mount(&server)
        .await;
    let cfg = config_for(&server);
    let ranked = co_listed_counts(&cfg, &[1, 2], 714600).await.unwrap();
    assert_eq!(ranked, vec![(200, 3), (201, 2), (300, 1)]);
}

#[tokio::test]
async fn fetch_candidates_preserves_rank_order_and_attaches_counts() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_in:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [
                { "id": 201, "slug": "b201", "title": "Beta",
                  "contributions": [{ "author": { "name": "B" } }], "book_series": [], "image": null },
                { "id": 200, "slug": "b200", "title": "Alpha",
                  "contributions": [{ "author": { "name": "A" } }], "book_series": [], "image": null }
            ] }
        })))
        .mount(&server)
        .await;
    let cfg = config_for(&server);
    let cands = fetch_candidates(&cfg, &[(200, 5), (201, 3)]).await.unwrap();
    assert_eq!(cands[0].hardcover_id, 200);
    assert_eq!(cands[0].list_count, 5);
    assert_eq!(cands[1].hardcover_id, 201);
}

#[tokio::test]
async fn book_description_returns_trimmed_description_for_a_resolved_book_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("description"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{ "description": "  A clever fox outwits three farmers.  " }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let got = book_description(&cfg, 714600).await.unwrap();
    assert_eq!(got.as_deref(), Some("A clever fox outwits three farmers."));
}

#[tokio::test]
async fn book_description_returns_none_when_resolved_book_has_a_blank_description() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(body_string_contains("description"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{ "description": "   " }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let got = book_description(&cfg, 714600).await.unwrap();
    assert!(got.is_none());
}

#[tokio::test]
async fn book_description_propagates_http_error_when_server_returns_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let err = book_description(&cfg, 714600).await.unwrap_err();
    assert!(matches!(err, HardcoverError::Http(_)));
}

#[tokio::test]
async fn book_description_propagates_graphql_error_when_response_carries_errors_array() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "errors": [{ "message": "field 'description' not found" }]
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let err = book_description(&cfg, 714600).await.unwrap_err();
    assert!(
        matches!(&err, HardcoverError::Graphql(msg) if msg.contains("field 'description' not found")),
        "got {err:?}"
    );
}

#[tokio::test]
async fn resolve_book_propagates_http_error_when_server_returns_500() {
    // `error_for_status()` turns a 5xx response into a `reqwest::Error`,
    // which `#[from]` maps to `HardcoverError::Http` — the same variant a
    // transport-level failure would produce, but deterministic and
    // instant via the existing `wiremock` harness instead of depending on
    // a specific port being unreachable.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let err = resolve_book(&cfg, &["9780000000000".to_string()], "Some Title", None)
        .await
        .unwrap_err();
    assert!(matches!(err, HardcoverError::Http(_)));
}

#[tokio::test]
async fn resolve_book_propagates_an_earlier_isbn_error_even_when_a_later_isbn_in_the_same_chunk_would_have_resolved(
) {
    // ISBN_RESOLVE_CONCURRENCY == 4; 6 ISBNs span two `chunks()` rounds (4 +
    // 2). The concurrent per-chunk fetch must still preserve the old
    // sequential loop's priority-order semantics: walk results in ISBN
    // order and stop at the first one that either resolves OR errors.
    // isbn[1] (2nd priority, same chunk as isbn[2]'s would-be winner) errors
    // — the walk must surface that error rather than skipping past it to
    // isbn[2]'s eventual match, proving concurrency didn't quietly change
    // "first-in-priority-order wins" into "first-successful-response wins".
    let server = MockServer::start().await;
    let isbns = [
        "9780000000000", // miss
        "9780000000001", // errors (500) — must win priority over isbn[2]
        "9780000000002", // would resolve to book_id 777, but unreached
        "9780000000003", // miss, same chunk
        "9780000000004", // miss, 2nd chunk
        "9780000000005", // miss, 2nd chunk
    ];
    for isbn in isbns {
        let response = if isbn == "9780000000001" {
            ResponseTemplate::new(500)
        } else if isbn == "9780000000002" {
            ResponseTemplate::new(200).set_body_json(json!({
                "data": { "editions": [{ "book_id": 777 }] }
            }))
        } else {
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "editions": [] } }))
        };
        Mock::given(method("POST"))
            .and(body_string_contains(isbn))
            .respond_with(response)
            .mount(&server)
            .await;
    }
    // isbn[2]'s edition lookup resolves to book_id 777, so `resolve_by_isbn`
    // follows up with this detail fetch as part of the SAME concurrent
    // `join_all` chunk that also runs isbn[1]'s failing request — stubbed so
    // isbn[2] genuinely completes as a would-be winner, not an unmatched
    // (and therefore also-erroring) request that would prove nothing.
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_eq: 777}}"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [{
                "id": 777, "slug": "would-be-winner", "title": "Would Be Winner",
                "contributions": [], "book_series": [], "image": null
            }] }
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let err = resolve_book(
        &cfg,
        &isbns.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        "Irrelevant Title",
        None,
    )
    .await
    .expect_err("an earlier-priority ISBN's error must propagate");
    assert!(
        matches!(err, HardcoverError::Http(_)),
        "expected the 500 from isbn[1] to surface as Http, got {err:?}"
    );

    // Every ISBN's edition lookup must have fired — the eager per-chunk
    // `join_all` fetches the whole list before the priority walk runs, so
    // both chunks (4 + 2) complete even though the walk aborts at isbn[1].
    let requests = server.received_requests().await.unwrap();
    let edition_lookups = requests
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("editions(where:"))
        .count();
    assert_eq!(
        edition_lookups, 6,
        "all 6 ISBNs across both concurrency chunks must have been queried"
    );
    // isbn[2]'s book-777 detail fetch must have actually fired — proving it
    // was a genuine, fully-resolved would-be winner that the earlier error
    // beat, not a mock gap that made the "earlier error wins" assertion
    // vacuously true.
    let book_777_fetches = requests
        .iter()
        .filter(|r| String::from_utf8_lossy(&r.body).contains("books(where: {id: {_eq: 777}}"))
        .count();
    assert_eq!(
        book_777_fetches, 1,
        "isbn[2] must have fully resolved to book 777 in the background, even though \
         the earlier isbn[1] error is what the walk actually surfaces"
    );
}
