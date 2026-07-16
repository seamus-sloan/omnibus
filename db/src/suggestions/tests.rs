//! Tests for the suggestions module: the pure cache state machine and
//! filters, the cache CRUD, the Hardcover GraphQL client (against `wiremock`),
//! and an end-to-end cascade run.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::author_photos::RemoteImageConfig;
use crate::pool::init_db;
use crate::suggestions::cascade::resolve_with;
use crate::suggestions::data::{
    decide, delete_suggestions, get_suggestion_cover, get_suggestions, mark_pending,
    replace_suggestions, suggestion_state, NewSuggestion, SuggestionState, SuggestionsDataError,
    PENDING_DEBOUNCE_SECS, SUGGESTIONS_TTL_SECS,
};
use crate::suggestions::filter::{
    filter_candidates, is_entry_point, is_same_author, is_same_series, Candidate,
};
use crate::suggestions::hardcover::{
    co_listed_counts, curated_list_ids, fetch_candidates, resolve_book, HardcoverConfig,
    HardcoverError,
};
use crate::test_support::seed_synced_ebook;

// ── decide(): the de-duplication state machine ───────────────────

#[test]
fn decide_enqueues_when_no_marker_exists() {
    let d = decide(None, 1_000);
    assert!(d.enqueue && !d.serve);
}

#[test]
fn decide_does_not_repost_a_recent_pending_marker() {
    let d = decide(
        Some((SuggestionState::Pending, 1_000)),
        1_000 + PENDING_DEBOUNCE_SECS - 1,
    );
    assert!(!d.enqueue && !d.serve);
}

#[test]
fn decide_reposts_a_stale_pending_marker() {
    let d = decide(
        Some((SuggestionState::Pending, 1_000)),
        1_000 + PENDING_DEBOUNCE_SECS + 1,
    );
    assert!(d.enqueue && !d.serve);
}

#[test]
fn decide_serves_fresh_resolved_without_reposting() {
    let d = decide(Some((SuggestionState::Resolved, 1_000)), 1_000 + 10);
    assert!(d.serve && !d.enqueue);
}

#[test]
fn decide_serves_and_refreshes_stale_resolved() {
    let d = decide(
        Some((SuggestionState::Resolved, 1_000)),
        1_000 + SUGGESTIONS_TTL_SECS + 1,
    );
    assert!(d.serve && d.enqueue);
}

#[test]
fn decide_keeps_sticky_empty_quiet_until_ttl() {
    let fresh = decide(Some((SuggestionState::Empty, 1_000)), 1_000 + 10);
    assert!(fresh.serve && !fresh.enqueue);
    let stale = decide(
        Some((SuggestionState::Empty, 1_000)),
        1_000 + SUGGESTIONS_TTL_SECS + 1,
    );
    assert!(stale.serve && stale.enqueue);
}

// ── filter: entry-point / same-author / same-series / trimming ────

#[test]
fn is_entry_point_passes_standalones_and_series_starters_only() {
    assert!(is_entry_point(None, None));
    assert!(is_entry_point(Some(""), None));
    assert!(is_entry_point(Some("The Empyrean"), Some(1.0)));
    assert!(is_entry_point(Some("The Empyrean"), Some(0.5)));
    assert!(!is_entry_point(Some("The Empyrean"), Some(2.0)));
    // A series book with unknown position is conservatively excluded.
    assert!(!is_entry_point(Some("The Empyrean"), None));
}

#[test]
fn same_author_and_series_compare_case_insensitively() {
    assert!(is_same_author(
        "rebecca yarros",
        &["Rebecca Yarros".to_string()]
    ));
    assert!(!is_same_author(
        "Brandon Sanderson",
        &["Rebecca Yarros".to_string()]
    ));
    assert!(is_same_series(Some("the empyrean"), Some("The Empyrean")));
    assert!(!is_same_series(Some("The Empyrean"), None));
    assert!(!is_same_series(None, None));
}

fn candidate(id: i64, author: &str, series: Option<&str>, pos: Option<f64>, lc: i64) -> Candidate {
    Candidate {
        hardcover_id: id,
        slug: Some(format!("b{id}")),
        title: format!("Title {id}"),
        author: author.to_string(),
        series: series.map(str::to_string),
        series_position: pos,
        list_count: lc,
        cover_url: None,
    }
}

#[test]
fn filter_candidates_excludes_author_series_nonstarters_and_dedupes() {
    let cands = vec![
        candidate(1, "Author A", None, None, 9), // keep — standalone, diff author
        candidate(1, "Author A", None, None, 9), // dup id — dropped
        candidate(2, "Source Author", None, None, 8), // same author — dropped
        candidate(3, "Author C", Some("Saga"), Some(3.0), 7), // mid-series — dropped
        candidate(4, "Author D", Some("Other"), Some(1.0), 6), // keep — series starter
        candidate(5, "Author E", Some("Trilogy"), None, 5), // unknown position — dropped
        candidate(6, "Author F", None, None, 4), // keep — standalone
    ];
    let source_authors = vec!["Source Author".to_string()];
    let out = filter_candidates(&cands, &source_authors, None, 2);
    // Limit 2, order preserved: ids 1 and 4.
    assert_eq!(
        out.iter().map(|c| c.hardcover_id).collect::<Vec<_>>(),
        vec![1, 4]
    );
}

#[test]
fn filter_candidates_excludes_same_series_as_source() {
    let cands = vec![
        candidate(1, "Author A", Some("The Empyrean"), Some(1.0), 9), // same series — dropped
        candidate(2, "Author B", None, None, 8),                      // keep
    ];
    let out = filter_candidates(&cands, &[], Some("The Empyrean"), 10);
    assert_eq!(
        out.iter().map(|c| c.hardcover_id).collect::<Vec<_>>(),
        vec![2]
    );
}

// ── cache CRUD ───────────────────────────────────────────────────

fn new_suggestion(id: i64, title: &str, cover: bool) -> NewSuggestion {
    NewSuggestion {
        hardcover_id: id,
        hardcover_slug: Some(format!("slug-{id}")),
        title: title.to_string(),
        author: "Some Author".to_string(),
        list_count: id,
        cover_mime: cover.then(|| "image/jpeg".to_string()),
        cover_bytes: cover.then(|| b"\xFF\xD8\xFFjpeg".to_vec()),
    }
}

#[tokio::test]
async fn mark_pending_sets_pending_state() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    mark_pending(&pool, "book-1").await.unwrap();
    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Pending);
}

#[tokio::test]
async fn replace_suggestions_persists_rows_and_resolved_state() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let rows = vec![
        new_suggestion(10, "First", true),
        new_suggestion(20, "Second", false),
    ];
    replace_suggestions(&pool, "book-1", &rows, SuggestionState::Resolved)
        .await
        .unwrap();

    let got = get_suggestions(&pool, "book-1").await.unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(got[0].rank, 0);
    assert_eq!(got[0].title, "First");
    assert!(got[0].has_cover);
    assert_eq!(got[1].title, "Second");
    assert!(!got[1].has_cover);

    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Resolved);

    let (mime, bytes) = get_suggestion_cover(&pool, "book-1", 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(mime, "image/jpeg");
    assert!(!bytes.is_empty());
    assert!(get_suggestion_cover(&pool, "book-1", 1)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn replace_suggestions_records_sticky_empty_marker() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(&pool, "book-1", &[], SuggestionState::Empty)
        .await
        .unwrap();
    assert!(get_suggestions(&pool, "book-1").await.unwrap().is_empty());
    let (state, _) = suggestion_state(&pool, "book-1").await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Empty);
}

#[tokio::test]
async fn replace_suggestions_overwrites_prior_rows() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(1, "Old", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(2, "New", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    let got = get_suggestions(&pool, "book-1").await.unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].title, "New");
}

#[tokio::test]
async fn delete_suggestions_clears_rows_and_marker() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_suggestions(
        &pool,
        "book-1",
        &[new_suggestion(1, "X", false)],
        SuggestionState::Resolved,
    )
    .await
    .unwrap();
    delete_suggestions(&pool, "book-1").await.unwrap();
    assert!(get_suggestions(&pool, "book-1").await.unwrap().is_empty());
    assert!(suggestion_state(&pool, "book-1").await.unwrap().is_none());
}

// ── Hardcover client (wiremock) ──────────────────────────────────

fn config_for(server: &MockServer) -> HardcoverConfig {
    HardcoverConfig {
        base_url: server.uri(),
        api_key: "test-key".to_string(),
        timeout: std::time::Duration::from_secs(5),
    }
}

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

// ── cascade end-to-end ───────────────────────────────────────────

#[tokio::test]
async fn resolve_with_caches_filtered_survivors_end_to_end() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;
    let cover_url = format!("{}/c200.jpg", server.uri());

    // 1. Title fallback resolves the source book.
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
    // 2. Curated lists.
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {book_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": [{ "list_id": 1 }, { "list_id": 2 }] }
        })))
        .mount(&server)
        .await;
    // 3. Co-listing members (counts: 200→3, 202→2, 201→2, 300→1).
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {list_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": [
                { "book_id": 200 }, { "book_id": 200 }, { "book_id": 200 },
                { "book_id": 202 }, { "book_id": 202 },
                { "book_id": 201 }, { "book_id": 201 },
                { "book_id": 300 },
                { "book_id": 100 }
            ] }
        })))
        .mount(&server)
        .await;
    // 4. Candidate details: 200 standalone (keep+cover), 201 same author (drop),
    //    300 mid-series (drop), 202 series-starter (keep, no cover).
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_in:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "books": [
                { "id": 200, "slug": "alpha", "title": "Alpha",
                  "contributions": [{ "author": { "name": "Author A" } }],
                  "book_series": [], "image": { "url": cover_url } },
                { "id": 201, "slug": "beta", "title": "Beta",
                  "contributions": [{ "author": { "name": "Source Author" } }],
                  "book_series": [], "image": null },
                { "id": 300, "slug": "gamma", "title": "Gamma",
                  "contributions": [{ "author": { "name": "Author C" } }],
                  "book_series": [{ "position": 3, "series": { "name": "Saga" } }], "image": null },
                { "id": 202, "slug": "delta", "title": "Delta",
                  "contributions": [{ "author": { "name": "Author D" } }],
                  "book_series": [{ "position": 1, "series": { "name": "Other" } }], "image": null }
            ] }
        })))
        .mount(&server)
        .await;
    // 5. Cover image fetch for book 200.
    Mock::given(method("GET"))
        .and(body_string_contains(""))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"\xFF\xD8\xFFcoverbytes".to_vec()),
        )
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let image_cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    resolve_with(&pool, &uuid, &cfg, &image_cfg).await.unwrap();

    let got = get_suggestions(&pool, &uuid).await.unwrap();
    assert_eq!(
        got.iter().map(|s| s.title.as_str()).collect::<Vec<_>>(),
        vec!["Alpha", "Delta"]
    );
    assert_eq!(got[0].list_count, 3);
    assert!(got[0].has_cover);
    assert_eq!(got[1].list_count, 2);
    assert!(!got[1].has_cover);

    let (state, _) = suggestion_state(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(state, SuggestionState::Resolved);
}

#[tokio::test]
async fn resolve_with_hydrates_covers_across_multiple_concurrency_chunks_with_partial_failure() {
    // COVER_FETCH_CONCURRENCY == 6; 8 survivors span two `chunks()` rounds
    // (6 + 2). One cover in the FIRST chunk (book 204) deliberately 404s to
    // prove a single failure doesn't drop, misorder, or poison its
    // chunk-mates — the other 5 covers in that same chunk (plus both in the
    // second chunk) must still hydrate correctly.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_synced_ebook(&pool, "src.epub", "Test Source", "Source Author").await;

    let server = MockServer::start().await;

    // 1. Title fallback resolves the source book.
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
    // 2. Curated lists.
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {book_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": [{ "list_id": 1 }] }
        })))
        .mount(&server)
        .await;
    // 3. Co-listing members: 8 distinct candidates (201..=208) with strictly
    //    descending counts (9..=2) so the rank order is deterministic.
    let mut member_rows = vec![json!({ "book_id": 100 })]; // source book — excluded
    for (id, count) in (201..=208).zip((2..=9).rev()) {
        for _ in 0..count {
            member_rows.push(json!({ "book_id": id }));
        }
    }
    Mock::given(method("POST"))
        .and(body_string_contains("list_books(where: {list_id:"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": { "list_books": member_rows }
        })))
        .mount(&server)
        .await;
    // 4. Candidate details: 8 different authors, no series (all survive the
    //    filter), each with its own cover URL.
    let books: Vec<serde_json::Value> = (201..=208)
        .map(|id: i64| {
            let cover_url = format!("{}/cover{id}.jpg", server.uri());
            json!({
                "id": id, "slug": format!("b{id}"), "title": format!("Title {id}"),
                "contributions": [{ "author": { "name": format!("Author {id}") } }],
                "book_series": [], "image": { "url": cover_url }
            })
        })
        .collect();
    Mock::given(method("POST"))
        .and(body_string_contains("books(where: {id: {_in:"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(json!({ "data": { "books": books } })),
        )
        .mount(&server)
        .await;
    // 5. Cover fetches — 7 succeed; book 204 (list_count 6, in the first
    //    chunk of 201..=206) 404s.
    for id in 201..=208 {
        let route = format!("/cover{id}.jpg");
        if id == 204 {
            Mock::given(method("GET"))
                .and(wiremock::matchers::path(route))
                .respond_with(ResponseTemplate::new(404))
                .mount(&server)
                .await;
        } else {
            Mock::given(method("GET"))
                .and(wiremock::matchers::path(route))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("content-type", "image/jpeg")
                        .set_body_bytes(vec![0xFFu8, 0xD8, 0xFF, id as u8]),
                )
                .mount(&server)
                .await;
        }
    }

    let cfg = config_for(&server);
    let image_cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    resolve_with(&pool, &uuid, &cfg, &image_cfg).await.unwrap();

    let got = get_suggestions(&pool, &uuid).await.unwrap();
    assert_eq!(
        got.len(),
        8,
        "all 8 survivors must be persisted, across both concurrency chunks"
    );
    let ids: Vec<i64> = got.iter().map(|s| s.hardcover_id).collect();
    assert_eq!(
        ids,
        vec![201, 202, 203, 204, 205, 206, 207, 208],
        "rank order (by co-listing count desc) must survive the chunked cover fetch"
    );
    for s in &got {
        if s.hardcover_id == 204 {
            assert!(
                !s.has_cover,
                "the 404'd cover must not be recorded as present"
            );
        } else {
            assert!(
                s.has_cover,
                "book {}'s cover must survive despite a sibling's cover failing",
                s.hardcover_id
            );
        }
    }
}

#[tokio::test]
async fn mark_pending_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = mark_pending(&pool, "any-uuid").await.unwrap_err();
    assert!(matches!(err, SuggestionsDataError::Db(_)));
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
