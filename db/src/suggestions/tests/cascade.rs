//! The end-to-end cascade: `resolve_with` caching the filtered survivors
//! and hydrating covers across concurrency chunks with partial failure,
//! and `extract_isbns` by identifier scheme or digit heuristic.

use serde_json::json;
use wiremock::matchers::{body_string_contains, method};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::config_for;
use crate::author_photos::RemoteImageConfig;
use crate::pool::init_db;
use crate::suggestions::cascade::{extract_isbns, resolve_with};
use crate::suggestions::data::{get_suggestions, suggestion_state, SuggestionState};
use crate::test_support::seed_synced_ebook;

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
        ..RemoteImageConfig::default()
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
        ..RemoteImageConfig::default()
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

#[test]
fn extract_isbns_matches_identifiers_whose_scheme_names_isbn() {
    let book = omnibus_shared::EbookMetadata {
        identifiers: vec![
            omnibus_shared::Identifier {
                value: "978-0-13-468599-1".into(),
                scheme: Some("ISBN".into()),
            },
            // A differently-schemed identifier with no digits at all must
            // still be excluded (cleaned value is empty).
            omnibus_shared::Identifier {
                value: "not-an-isbn".into(),
                scheme: Some("mobi-asin".into()),
            },
        ],
        ..Default::default()
    };
    let isbns = extract_isbns(&book);
    assert_eq!(
        isbns,
        vec!["9780134685991".to_string()],
        "scheme-named ISBN identifier should survive with hyphens stripped"
    );
}

#[test]
fn extract_isbns_matches_bare_digit_values_via_heuristic_when_scheme_is_unrecognized() {
    let book = omnibus_shared::EbookMetadata {
        identifiers: vec![
            // Common `scheme = "unknown"` case: no "isbn" in the scheme name,
            // but the value is a bare 10-digit string.
            omnibus_shared::Identifier {
                value: "0141439513".into(),
                scheme: Some("unknown".into()),
            },
            // Too short to look like an ISBN and not scheme-named — excluded.
            omnibus_shared::Identifier {
                value: "12345".into(),
                scheme: None,
            },
        ],
        ..Default::default()
    };
    let isbns = extract_isbns(&book);
    assert_eq!(
        isbns,
        vec!["0141439513".to_string()],
        "10-digit bare value should be picked up by the length heuristic alone"
    );
}
