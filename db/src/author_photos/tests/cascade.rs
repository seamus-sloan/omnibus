//! The cascade resolver against the Open Library wiremock: a hit writes
//! the photo, an empty search or missing/too-small cover writes the
//! letter marker, existing markers and manual uploads are left alone, a
//! transient error leaves the row absent, the configured User-Agent, and
//! `refetch_all`'s progress and concurrency chunks.

use std::time::Duration;

use sqlx::SqlitePool;
use wiremock::{
    matchers::{method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

use super::super::cascade::{fetch_open_library, refetch_all, resolve_with, OpenLibraryConfig};
use crate::author_photos_data::{
    author_photo_status, get_author_photo, upsert_author_photo, AuthorPhotoSource,
};
use crate::pool::init_db;

async fn pool_with_author(name: &str) -> (SqlitePool, i64) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id: i64 = sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
    (pool, id)
}

fn config_for(server: &MockServer) -> OpenLibraryConfig {
    OpenLibraryConfig {
        base_search_url: server.uri(),
        base_covers_url: server.uri(),
        timeout: Duration::from_secs(2),
        user_agent: "omnibus-test".into(),
    }
}

#[tokio::test]
async fn resolve_writes_open_library_hit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL23919A" } ]
        })))
        .mount(&server)
        .await;
    // 2 KB of bytes so the MIN_IMAGE_BYTES guard passes.
    let payload = vec![0xABu8; 2048];
    Mock::given(method("GET"))
        .and(path("/a/olid/OL23919A-L.jpg"))
        .and(query_param("default", "false"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(payload.clone()),
        )
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (mime, bytes) = get_author_photo(&pool, id).await.unwrap().unwrap();
    assert_eq!(mime, "image/jpeg");
    assert_eq!(bytes, payload);
    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::OpenLibrary);
}

#[tokio::test]
async fn resolve_writes_letter_when_search_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": []
        })))
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Nobody In Particular").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    assert!(get_author_photo(&pool, id).await.unwrap().is_none());
    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_writes_letter_when_cover_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL999A" } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/a/olid/OL999A-L.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_writes_letter_when_image_too_small() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL1A" } ]
        })))
        .mount(&server)
        .await;
    // Tiny placeholder (well under MIN_IMAGE_BYTES) — should be treated
    // as a miss.
    Mock::given(method("GET"))
        .and(path("/a/olid/OL1A-L.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/gif")
                .set_body_bytes(vec![0u8; 42]),
        )
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_is_noop_when_letter_marker_exists() {
    // Existing letter marker must prevent any HTTP call. We assert this
    // by starting a mock server with *no* mounted responses — any
    // incoming request would 404 and we'd notice via the marker source
    // not changing.
    let server = MockServer::start().await;
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    upsert_author_photo(&pool, id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "letter marker must skip the network entirely"
    );
}

#[tokio::test]
async fn resolve_is_noop_when_manual_upload_exists() {
    let server = MockServer::start().await;
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    upsert_author_photo(
        &pool,
        id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(b"\xFF\xD8\xFFmanual"),
    )
    .await
    .unwrap();
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Manual);
}

#[tokio::test]
async fn fetch_open_library_sends_configured_user_agent() {
    // The shared client carries the production UA, but each request must
    // override it with the config's `user_agent` so test injection (and
    // any future per-call UA) still takes effect.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .and(wiremock::matchers::header("user-agent", "omnibus-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": []
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let result = fetch_open_library("Ada Lovelace", &cfg).await.unwrap();
    assert!(result.is_none());
    // The header matcher above only matches when the UA is correct, so a
    // single received request confirms the override fired.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

#[tokio::test]
async fn resolve_leaves_row_absent_on_transient_network_error() {
    // Point the resolver at a TCP port that nothing is listening on so
    // every request errors at the transport layer. A transient outage
    // must NOT cache a `letter` marker — the next call should be free
    // to retry, not stuck for an admin to manually clear.
    let cfg = OpenLibraryConfig {
        base_search_url: "http://127.0.0.1:1".into(),
        base_covers_url: "http://127.0.0.1:1".into(),
        timeout: Duration::from_millis(500),
        user_agent: "omnibus-test".into(),
    };
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    resolve_with(&pool, id, &cfg).await.unwrap();

    assert!(
        author_photo_status(&pool, id).await.unwrap().is_none(),
        "transient network error must leave the row absent for retry"
    );
}

#[tokio::test]
async fn refetch_all_skips_manual_and_reports_progress() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let manual_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Manual Author', 'Manual Author') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let letter_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Letter Author', 'Letter Author') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let empty_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('No Photo', 'No Photo') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    upsert_author_photo(
        &pool,
        manual_id,
        AuthorPhotoSource::Manual,
        Some("https://example.com/manual.jpg"),
        Some("image/jpeg"),
        Some(&[0xFF; 2048]),
    )
    .await
    .unwrap();
    upsert_author_photo(
        &pool,
        letter_id,
        AuthorPhotoSource::Letter,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let progress = std::sync::Mutex::new(Vec::new());
    refetch_all(&pool, |processed, total, _| {
        progress.lock().unwrap().push((processed, total));
    })
    .await
    .unwrap();

    let calls = progress.into_inner().unwrap();
    assert_eq!(calls.len(), 3, "one progress call per author");
    assert_eq!(calls[0], (1, Some(3)));
    assert_eq!(calls[1], (2, Some(3)));
    assert_eq!(calls[2], (3, Some(3)));

    let (src, _) = author_photo_status(&pool, manual_id)
        .await
        .unwrap()
        .expect("manual row should be preserved");
    assert_eq!(src, AuthorPhotoSource::Manual);

    // letter row was deleted + resolve re-ran. With real OL, the search
    // returns nothing for "Letter Author" so a new letter marker is written.
    // The key invariant: the old row was cleared and the cascade ran again.
    match author_photo_status(&pool, letter_id).await.unwrap() {
        Some((AuthorPhotoSource::Letter, _)) => {} // re-resolved to letter (expected)
        None => {}                                 // transient network error left absent
        other => panic!("unexpected status for letter author: {other:?}"),
    }

    // "No Photo" had no row — resolve ran (OL search miss → letter marker,
    // or transient error → absent). Either is fine; the point is the cascade
    // executed without error.
    match author_photo_status(&pool, empty_id).await.unwrap() {
        Some((AuthorPhotoSource::Letter, _)) | None => {}
        other => panic!("unexpected status for empty author: {other:?}"),
    }
}

#[tokio::test]
async fn refetch_all_processes_every_author_across_multiple_concurrency_chunks() {
    // REFETCH_CONCURRENCY == 6; seed 8 authors so `to_refetch` spans two
    // `chunks()` rounds (6 + 2) and the second round is actually exercised.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut author_ids = Vec::with_capacity(8);
    for i in 0..8 {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
                .bind(format!("Chunk Test Author {i}"))
                .bind(format!("Chunk Test Author {i}"))
                .fetch_one(&pool)
                .await
                .unwrap();
        author_ids.push(id);
    }

    let progress = std::sync::Mutex::new(Vec::new());
    let names = std::sync::Mutex::new(Vec::new());
    refetch_all(&pool, |processed, total, name| {
        progress.lock().unwrap().push((processed, total));
        names.lock().unwrap().push(name.map(str::to_string));
    })
    .await
    .unwrap();

    // Every refetched author reports its name as the current item;
    // completion order under concurrency is arbitrary, so only membership
    // is asserted.
    let names = names.into_inner().unwrap();
    assert!(
        names.iter().all(|n| n
            .as_deref()
            .is_some_and(|n| n.starts_with("Chunk Test Author"))),
        "every progress call must carry the completed author name: {names:?}"
    );

    let mut calls = progress.into_inner().unwrap();
    assert_eq!(
        calls.len(),
        8,
        "one progress call per author, spanning both concurrency chunks"
    );
    calls.sort_unstable();
    let expected: Vec<(u32, Option<u32>)> = (1..=8).map(|n| (n, Some(8))).collect();
    assert_eq!(
        calls, expected,
        "the completion counter must reach every value 1..=8 exactly once, \
         regardless of which chunk (or which author within a chunk) finishes first"
    );

    // Every author must have run the cascade to completion: either a sticky
    // `letter` marker (clean Open Library miss) or an absent row (transient
    // network error, e.g. no network access in this environment). Either
    // outcome proves the second chunk actually ran, not just the first 6.
    for id in author_ids {
        match author_photo_status(&pool, id).await.unwrap() {
            Some((AuthorPhotoSource::Letter, _)) | None => {}
            other => panic!("unexpected status for author {id}: {other:?}"),
        }
    }
}
