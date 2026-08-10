//! HTTP-layer contract tests for `/opds/*`, driving `opds_router` directly
//! (like `kobo/tests.rs`, since it's mounted outside `rest_router`) via
//! `oneshot` against an in-memory DB.

use axum::{body::to_bytes, http::StatusCode, Router};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use omnibus_shared::Settings;
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::{get_anon, get_with_bearer};

/// `opds_router` wired to a fresh in-memory DB, plus a bearer token for a
/// freshly created user. Settings point the ebook library at `/ebooks` —
/// the path [`seed_synced_ebook`] indexes under.
async fn fixture() -> (Router, SqlitePool, String) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let user = auth_test_support::create_user(&pool, "opds-reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = opds_router(AppState::new(pool.clone()));
    (app, pool, token)
}

async fn body_string(res: axum::response::Response) -> String {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// Index one ebook under an arbitrary scan root, so a test can give an
/// author books in a second library. `seed_synced_ebook` is hard-wired to
/// `/ebooks` (the fixture's configured ebook library) and can't.
async fn seed_ebook_in_library(
    pool: &SqlitePool,
    library_path: &str,
    filename: &str,
    title: &str,
    author: &str,
) {
    db::sync_books(
        pool,
        library_path,
        db::SyncPlan {
            new_books: vec![db::test_support::indexed(
                filename,
                Some(title),
                &[author],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_opds_route_401s_without_a_session_and_offers_a_basic_challenge() {
    // AC4: every /opds route requires authentication — and the 401 carries
    // a `WWW-Authenticate: Basic` challenge so an OPDS client knows to
    // retry with credentials.
    let (app, _pool, _token) = fixture().await;
    for uri in [
        "/opds",
        "/opds/osd",
        "/opds/search?q=x",
        "/opds/new",
        "/opds/authors",
        "/opds/authors/A",
        "/opds/author/1",
        "/opds/covers/some-uuid",
        "/opds/thumbs/some-uuid/sm",
        "/opds/ebooks/some-uuid/file",
        "/opds/ebooks/some-uuid/download",
        "/opds/audiobooks/some-uuid/download",
        "/opds/shelves",
        "/opds/shelves/1",
        "/opds/v2/shelves",
        "/opds/v2/shelves/1",
    ] {
        let res = app.clone().oneshot(get_anon(uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
        let challenge = res
            .headers()
            .get(axum::http::header::WWW_AUTHENTICATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default();
        assert!(
            challenge.starts_with("Basic "),
            "uri={uri} challenge={challenge:?}"
        );
    }
}

#[tokio::test]
async fn root_feed_is_a_navigation_feed_with_search_new_and_authors_links() {
    // AC1: GET /opds returns a valid OPDS 1.2 Atom navigation feed with
    // links to search, new, and the author browse.
    let (app, _pool, token) = fixture().await;
    let res = app.oneshot(get_with_bearer("/opds", &token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::NAVIGATION_TYPE)
    );
    let body = body_string(res).await;
    assert!(body.starts_with(r#"<?xml version="1.0" encoding="UTF-8"?>"#));
    assert!(body.contains(r#"xmlns="http://www.w3.org/2005/Atom""#));
    assert!(body.contains(r#"rel="search" href="/opds/osd""#));
    assert!(body.contains(r#"href="/opds/new""#));
    assert!(body.contains(r#"href="/opds/authors""#));
    assert!(body.contains(r#"href="/opds/all""#));
    assert!(body.contains(r#"href="/opds/series""#));
    assert!(body.contains(r#"href="/opds/shelves""#));
}

#[tokio::test]
async fn osd_returns_an_opensearch_description_pointing_at_opds_search() {
    // AC3: /opds/osd returns a valid OpenSearch description.
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/osd", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::OPENSEARCH_TYPE)
    );
    let body = body_string(res).await;
    assert!(body.contains(r#"xmlns="http://a9.com/-/spec/opensearch/1.1/""#));
    assert!(body.contains("template=\"/opds/search?q={searchTerms}\""));
}

#[tokio::test]
async fn search_returns_matching_acquisition_entries_for_a_query() {
    // AC3: /opds/search returns matching acquisition entries for a query.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/search?q=dune", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::ACQUISITION_TYPE)
    );
    let body = body_string(res).await;
    assert!(body.contains("<title>Dune</title>"));
    assert!(body.contains(r#"rel="http://opds-spec.org/acquisition""#));
}

#[tokio::test]
async fn search_with_no_matches_returns_an_empty_acquisition_feed() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/search?q=nonexistent", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(!body.contains("<entry>"));
}

#[tokio::test]
async fn new_arrivals_lists_a_recently_indexed_book() {
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/new", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("<title>Dune</title>"));
}

#[tokio::test]
async fn authors_letter_index_lists_the_seeded_authors_letter() {
    // AC2: the letter-indexed author browse.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/authors", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::NAVIGATION_TYPE)
    );
    let body = body_string(res).await;
    // "Frank Herbert" sorts under F.
    assert!(body.contains(r#"href="/opds/authors/F""#));
}

#[tokio::test]
async fn authors_letter_index_percent_encodes_the_hash_bucket_href() {
    // A `#` in an href is a URL fragment and never reaches the server, so
    // the "everything else" bucket must be percent-encoded as `%23`.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "123.epub", "One Two Three", "123 Collective").await;

    let res = app
        .oneshot(get_with_bearer("/opds/authors", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains(r#"href="/opds/authors/%23""#));
    assert!(!body.contains(r#"href="/opds/authors/#""#));
}

#[tokio::test]
async fn authors_by_letter_lists_the_authors_acquisition_feed_link() {
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/authors/F", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("<title>Frank Herbert</title>"));
    assert!(body.contains(&format!("href=\"/opds/author/{author_id}\"")));
}

#[tokio::test]
async fn authors_by_letter_rejects_a_multi_character_letter() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/authors/AB", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn author_acquisition_feed_includes_download_and_cover_links() {
    // AC2 + AC4: per-entity acquisition feeds return well-formed Atom with
    // working download and cover links.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/author/{author_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::ACQUISITION_TYPE)
    );
    let body = body_string(res).await;
    assert!(body.contains("<title>Dune</title>"));
    assert!(body.contains(r#"rel="http://opds-spec.org/acquisition""#));
    assert!(body.contains("/download\" type=\"application/epub+zip\""));
    assert!(body.contains(r#"rel="http://opds-spec.org/image""#));
    assert!(body.contains(r#"rel="http://opds-spec.org/image/thumbnail""#));
    // The byte-serving links must point at this router's Basic-auth'd
    // delegates, not the cookie/bearer-only `/api/*` originals — an OPDS
    // client replays the catalog credentials on the links it follows.
    assert!(body.contains(r#"href="/opds/ebooks/"#));
    assert!(body.contains(r#"href="/opds/covers/"#));
    assert!(body.contains(r#"href="/opds/thumbs/"#));
    assert!(!body.contains(r#"href="/api/"#));
}

#[tokio::test]
async fn author_acquisition_feed_omits_a_book_indexed_outside_the_ebook_library() {
    // The catalog is ebook-library scoped (see the `opds` module doc), so an
    // epub the author also has under another scan root must not appear —
    // format alone doesn't make it in-scope.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_ebook_in_library(
        &pool,
        "/elsewhere",
        "other.epub",
        "Elsewhere",
        "Frank Herbert",
    )
    .await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/author/{author_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("<title>Dune</title>"));
    assert!(!body.contains("<title>Elsewhere</title>"));
    assert_eq!(body.matches("<entry>").count(), 1);
}

#[tokio::test]
async fn author_acquisition_feed_returns_404_for_an_unknown_author_id() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/author/999999", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------------------
// OPDS 2.0 JSON catalog (`/opds/v2/*`)
// ---------------------------------------------------------------------------

/// Seed one ebook (like `seed_synced_ebook`) with subjects and a series too,
/// so the series-browse and category-parity tests have something to
/// browse/assert on. Local to this file — no other `opds` test needs it.
async fn seed_synced_ebook_with_series(
    pool: &SqlitePool,
    filename: &str,
    title: &str,
    author: &str,
    subjects: &[&str],
    series: (&str, &str),
) {
    db::sync_books(
        pool,
        "/ebooks",
        db::SyncPlan {
            new_books: vec![db::test_support::indexed(
                filename,
                Some(title),
                &[author],
                subjects,
                Some(series),
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn body_json(res: axum::response::Response) -> serde_json::Value {
    serde_json::from_str(&body_string(res).await).unwrap()
}

#[tokio::test]
async fn every_opds_v2_route_401s_without_a_session() {
    // AC3: every /opds/v2 route requires authentication.
    let (app, _pool, _token) = fixture().await;
    for uri in [
        "/opds/v2",
        "/opds/v2/search?q=x",
        "/opds/v2/new",
        "/opds/v2/authors",
        "/opds/v2/authors/A",
        "/opds/v2/author/1",
        "/opds/v2/series",
        "/opds/v2/series/1",
    ] {
        let res = app.clone().oneshot(get_anon(uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
    }
}

#[tokio::test]
async fn v2_root_feed_is_opds_json_with_search_new_authors_and_series_navigation() {
    // AC1: GET /opds/v2 returns application/opds+json with metadata, links,
    // and navigation into search, new, authors, and series.
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/v2", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(opds2::MEDIA_TYPE)
    );
    let json = body_json(res).await;
    assert_eq!(json["metadata"]["title"], "Omnibus");
    assert!(json["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["rel"] == "search" && l["templated"] == true));
    let nav = json["navigation"].as_array().unwrap();
    assert!(nav.iter().any(|l| l["href"] == "/opds/v2/new"));
    assert!(nav.iter().any(|l| l["href"] == "/opds/v2/authors"));
    assert!(nav.iter().any(|l| l["href"] == "/opds/v2/series"));
    assert!(nav.iter().any(|l| l["href"] == "/opds/v2/all"));
    assert!(nav.iter().any(|l| l["href"] == "/opds/v2/shelves"));
    assert!(json.get("publications").is_none());
}

#[tokio::test]
async fn v2_new_arrivals_lists_a_recently_indexed_book() {
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/v2/new", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0]["metadata"]["title"], "Dune");
}

#[tokio::test]
async fn v2_search_returns_matching_publications_for_a_query() {
    // AC1/AC2: /opds/v2/search returns matching publications for a query.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/v2/search?q=dune", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0]["metadata"]["title"], "Dune");
    assert!(pubs[0]["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/acquisition"));
}

#[tokio::test]
async fn v2_search_with_no_matches_returns_an_empty_publications_list() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/v2/search?q=nonexistent", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert!(json.get("publications").is_none());
}

#[tokio::test]
async fn v2_authors_letter_index_lists_the_seeded_authors_letter() {
    // AC2: the letter-indexed author browse, same shape as the Atom one.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer("/opds/v2/authors", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    // "Frank Herbert" sorts under F.
    assert!(json["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["href"] == "/opds/v2/authors/F"));
}

#[tokio::test]
async fn v2_authors_by_letter_rejects_a_multi_character_letter() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/v2/authors/AB", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn v2_author_acquisition_feed_includes_download_and_cover_links() {
    // AC2 + AC3: per-author publication feeds carry working download and
    // cover links, matching the Atom acquisition feed's contents.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/v2/author/{author_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0]["metadata"]["title"], "Dune");
    let links = pubs[0]["links"].as_array().unwrap();
    assert!(links
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/acquisition"
            && l["type"] == "application/epub+zip"));
    let images = pubs[0]["images"].as_array().unwrap();
    assert!(images
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/image"));
    assert!(images
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/image/thumbnail"));
}

#[tokio::test]
async fn v2_author_acquisition_feed_omits_a_book_indexed_outside_the_ebook_library() {
    // Same ebook-library scoping as the Atom feed — the two catalogs read
    // the same author set.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_ebook_in_library(
        &pool,
        "/elsewhere",
        "other.epub",
        "Elsewhere",
        "Frank Herbert",
    )
    .await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/v2/author/{author_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0]["metadata"]["title"], "Dune");
}

#[tokio::test]
async fn v2_author_acquisition_feed_returns_404_for_an_unknown_author_id() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/v2/author/999999", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v2_series_index_lists_a_seeded_series() {
    // AC2: series parity — a browse the Atom catalog has no equivalent of.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook_with_series(
        &pool,
        "dune.epub",
        "Dune",
        "Frank Herbert",
        &["Science Fiction"],
        ("Dune Saga", "1"),
    )
    .await;

    let res = app
        .oneshot(get_with_bearer("/opds/v2/series", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert!(json["navigation"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["title"] == "Dune Saga"));
}

#[tokio::test]
async fn v2_series_acquisition_feed_includes_the_books_series_position_and_categories() {
    // AC2: a publication in this feed carries its series position and
    // category (subject) data inline — the JSON catalog's answer to
    // "series and categories" parity, which Atom has no browse for.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook_with_series(
        &pool,
        "dune.epub",
        "Dune",
        "Frank Herbert",
        &["Science Fiction"],
        ("Dune Saga", "1"),
    )
    .await;
    let series_id = series_id_by_name(&pool, "Dune Saga").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/v2/series/{series_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(pubs[0]["metadata"]["title"], "Dune");
    assert_eq!(
        pubs[0]["metadata"]["belongsTo"]["series"][0]["name"],
        "Dune Saga"
    );
    assert_eq!(
        pubs[0]["metadata"]["belongsTo"]["series"][0]["position"],
        1.0
    );
    assert_eq!(pubs[0]["metadata"]["subject"][0]["name"], "Science Fiction");
    let links = pubs[0]["links"].as_array().unwrap();
    assert!(links
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/acquisition"));
}

#[tokio::test]
async fn v2_series_acquisition_feed_serializes_a_non_finite_series_index_as_a_null_position() {
    // Regression: `series_index` is free-text and Calibre-sourced, so a
    // garbage value like "NaN" parses as a valid f64 — but serde_json can't
    // serialize a non-finite float, which would 500 the whole feed if
    // `series_ref` didn't filter it out before it reached the response.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook_with_series(
        &pool,
        "dune.epub",
        "Dune",
        "Frank Herbert",
        &["Science Fiction"],
        ("Dune Saga", "NaN"),
    )
    .await;
    let series_id = series_id_by_name(&pool, "Dune Saga").await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/v2/series/{series_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let pubs = json["publications"].as_array().unwrap();
    assert_eq!(pubs.len(), 1);
    assert_eq!(
        pubs[0]["metadata"]["belongsTo"]["series"][0]["name"],
        "Dune Saga"
    );
    assert!(pubs[0]["metadata"]["belongsTo"]["series"][0]["position"].is_null());
}

#[tokio::test]
async fn v2_series_acquisition_feed_returns_404_for_an_unknown_series_id() {
    let (app, _pool, token) = fixture().await;
    let res = app
        .oneshot(get_with_bearer("/opds/v2/series/999999", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn v2_new_arrivals_book_count_matches_the_atom_feeds_book_count() {
    // AC2: the JSON catalog exposes the same set of books as the Atom feed.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_synced_ebook(
        &pool,
        "gatsby.epub",
        "The Great Gatsby",
        "F. Scott Fitzgerald",
    )
    .await;

    let atom_res = app
        .clone()
        .oneshot(get_with_bearer("/opds/new", &token))
        .await
        .unwrap();
    let atom_body = body_string(atom_res).await;
    let atom_count = atom_body.matches("<entry>").count();

    let json_res = app
        .oneshot(get_with_bearer("/opds/v2/new", &token))
        .await
        .unwrap();
    let json = body_json(json_res).await;
    let json_count = json["publications"].as_array().unwrap().len();

    assert_eq!(atom_count, 2);
    assert_eq!(atom_count, json_count);
}

// ------------------------------------------------------------- HTTP Basic

/// GET with `Authorization: Basic` credentials, the only scheme OPDS
/// clients like KOReader can send.
fn get_with_basic(
    uri: &str,
    username: &str,
    password: &str,
) -> axum::http::Request<axum::body::Body> {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    let encoded = STANDARD.encode(format!("{username}:{password}"));
    axum::http::Request::builder()
        .uri(uri)
        .header(
            axum::http::header::AUTHORIZATION,
            format!("Basic {encoded}"),
        )
        .body(axum::body::Body::empty())
        .unwrap()
}

#[tokio::test]
async fn opds_routes_accept_valid_basic_credentials() {
    let (app, pool, _token) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", "opds-basic-pass-1").await;
    let res = app
        .oneshot(get_with_basic("/opds", "basic-reader", "opds-basic-pass-1"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some(atom::NAVIGATION_TYPE)
    );
}

#[tokio::test]
async fn basic_auth_rejects_a_wrong_password_with_a_challenge() {
    let (app, pool, _token) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", "opds-basic-pass-1").await;
    let res = app
        .oneshot(get_with_basic("/opds", "basic-reader", "wrong-password"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let challenge = res
        .headers()
        .get(axum::http::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(challenge.starts_with("Basic "), "challenge={challenge:?}");
}

#[tokio::test]
async fn basic_auth_caches_verified_credentials_across_requests() {
    // 12 valid requests > the limiter's 10-per-window budget: only the
    // first (uncached) verification consumes budget or pays Argon2, so all
    // twelve must succeed. A per-request verify would 429 at request 11.
    let (app, pool, _token) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", "opds-basic-pass-1").await;
    for i in 0..12 {
        let res = app
            .clone()
            .oneshot(get_with_basic("/opds", "basic-reader", "opds-basic-pass-1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "request {i}");
    }
}

#[tokio::test]
async fn basic_auth_rate_limits_repeated_failures_per_ip() {
    // AC4 (#1805): failed Basic attempts consume the same 10-per-minute
    // per-IP budget as the login endpoints. An unknown username never
    // reaches the per-account lockout, so every failure is limiter-metered.
    let (app, _pool, _token) = fixture().await;
    for i in 0..10 {
        let res = app
            .clone()
            .oneshot(get_with_basic("/opds", "ghost-user", "whatever"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "request {i}");
    }
    let res = app
        .oneshot(get_with_basic("/opds", "ghost-user", "whatever"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn basic_auth_surfaces_an_account_lockout_as_403() {
    // Five wrong passwords trip `verify_login`'s per-account lockout; the
    // right password then reports the lock rather than logging in, so a
    // client shows "locked" instead of silently retrying forever.
    let (app, pool, _token) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "locky", "opds-basic-pass-1").await;
    for _ in 0..5 {
        let res = app
            .clone()
            .oneshot(get_with_basic("/opds", "locky", "wrong-password"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
    let res = app
        .oneshot(get_with_basic("/opds", "locky", "opds-basic-pass-1"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn bearer_sessions_still_authenticate_after_the_basic_fallback_landed() {
    // AC3 (#1805): the session path is tried first and unchanged — a valid
    // bearer token must not be affected by the Basic fallback.
    let (app, _pool, token) = fixture().await;
    let res = app.oneshot(get_with_bearer("/opds", &token)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn download_delegate_serves_the_epub_with_basic_credentials() {
    // AC2 (#1805): the acquisition link a feed hands out works with the
    // same Basic credentials the catalog was browsed with.
    let (app, pool, _token) = fixture().await;
    auth_test_support::create_user_with_password(&pool, "basic-reader", "opds-basic-pass-1").await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_opds_download_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    std::fs::write(tmp.join("alpha.epub"), b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "44444444-4444-4444-4444-444444444444";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let res = app
        .oneshot(get_with_basic(
            &format!("/opds/ebooks/{uuid}/download"),
            "basic-reader",
            "opds-basic-pass-1",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let disposition = res
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        disposition.starts_with("attachment;"),
        "download must force an attachment disposition, got {disposition:?}"
    );
    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn acquisition_delegates_403_without_the_download_permission() {
    // AC2 (#1805): the download routes enforce `can_download`; covers and
    // thumbs deliberately don't — the catalog is unrenderable without its
    // images, matching the browse UI.
    let (app, pool, _token) = fixture().await;
    let user =
        auth_test_support::create_user_with_password(&pool, "no-dl", "opds-basic-pass-1").await;
    sqlx::query("UPDATE users SET can_download = 0 WHERE id = ?")
        .bind(user.id)
        .execute(&pool)
        .await
        .unwrap();

    for uri in [
        "/opds/ebooks/some-uuid/file",
        "/opds/ebooks/some-uuid/download",
        "/opds/audiobooks/some-uuid/download",
    ] {
        let res = app
            .clone()
            .oneshot(get_with_basic(uri, "no-dl", "opds-basic-pass-1"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN, "uri={uri}");
    }

    // Covers stay reachable: 404 (nothing seeded), never 401/403.
    let res = app
        .oneshot(get_with_basic(
            "/opds/covers/some-uuid",
            "no-dl",
            "opds-basic-pass-1",
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn physical_only_books_are_excluded_from_new_and_search_feeds() {
    // #1811: the shared list/search queries surface physical-only books on
    // purpose for the web UI (#1181), but a catalog entry with no
    // acquisition link is a dead row on an e-reader. The author and series
    // feeds already filtered; new and search must too.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let phys = db::physical::create_fileless_book(
        &pool,
        db::physical::FilelessBook {
            title: "Print Only Chronicle".into(),
            authors: vec!["Print Author".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    db::physical::add_physical_copy(&pool, &phys, None, None, None)
        .await
        .unwrap();

    // Sanity: the book IS visible to the web-facing list query — what the
    // feeds do below is OPDS-specific filtering, not general invisibility.
    let page = db::list_books_page(
        &pool,
        &["/ebooks"],
        omnibus_shared::SortKey::NewestAdded,
        omnibus_shared::SortDir::Desc,
        &omnibus_shared::ViewFilters::default(),
        &[],
        None,
        50,
    )
    .await
    .unwrap();
    assert!(
        page.books
            .iter()
            .any(|b| b.display_title() == "Print Only Chronicle"),
        "physical-only book must remain visible to the web list query"
    );

    // The author feeds filtered before this change — the per-author URIs
    // here are regression coverage so the shared predicate never lets a
    // physical-only book leak back in as a linkless entry.
    let author_id = author_id_by_name(&pool, "Print Author").await;
    for uri in [
        "/opds/new".to_string(),
        "/opds/search?q=chronicle".to_string(),
        "/opds/v2/new".to_string(),
        "/opds/v2/search?q=chronicle".to_string(),
        format!("/opds/author/{author_id}"),
        format!("/opds/v2/author/{author_id}"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "uri={uri}");
        let body = body_string(res).await;
        assert!(
            !body.contains("Print Only Chronicle"),
            "uri={uri} must exclude the fileless book"
        );
    }

    let res = app
        .oneshot(get_with_bearer("/opds/new", &token))
        .await
        .unwrap();
    assert!(
        body_string(res).await.contains("Dune"),
        "file-backed books must still flow through the feed"
    );
}

// ------------------------------------------------------- All Books + Series

/// Pull the `rel="next"` href out of an Atom feed body, if present.
fn next_href(body: &str) -> Option<String> {
    let marker = r#"rel="next" href=""#;
    let start = body.find(marker)? + marker.len();
    let rest = &body[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

#[tokio::test]
async fn all_books_pages_through_the_whole_library_with_rel_next() {
    // AC2 (#1812): the All Books feed walks the entire library via
    // rel="next" — 55 seeded books over a 50-row page cap means exactly two
    // pages, and the entry total across them must equal the library.
    let (app, pool, token) = fixture().await;
    for i in 0..55 {
        seed_synced_ebook(
            &pool,
            &format!("book{i:02}.epub"),
            &format!("Book {i:02}"),
            "Bulk Author",
        )
        .await;
    }

    let res = app
        .clone()
        .oneshot(get_with_bearer("/opds/all", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let page1 = body_string(res).await;
    assert_eq!(page1.matches("<entry>").count(), 50);
    let next = next_href(&page1).expect("page 1 must carry a rel=next link");

    let res = app
        .clone()
        .oneshot(get_with_bearer(&next, &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let page2 = body_string(res).await;
    assert_eq!(page2.matches("<entry>").count(), 5);
    assert!(
        next_href(&page2).is_none(),
        "the final page must not advertise a next link"
    );
    // Keyset paging: the pages must not overlap.
    assert!(page1.contains("<title>Book 00</title>"));
    assert!(page2.contains("<title>Book 54</title>"));
    assert!(!page2.contains("<title>Book 00</title>"));
}

#[tokio::test]
async fn v2_all_books_pages_like_the_atom_feed() {
    let (app, pool, token) = fixture().await;
    for i in 0..55 {
        seed_synced_ebook(
            &pool,
            &format!("book{i:02}.epub"),
            &format!("Book {i:02}"),
            "Bulk Author",
        )
        .await;
    }
    let res = app
        .clone()
        .oneshot(get_with_bearer("/opds/v2/all", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["publications"].as_array().unwrap().len(), 50);
    let next = json["links"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["rel"] == "next")
        .expect("page 1 must carry a next link")["href"]
        .as_str()
        .unwrap()
        .to_string();
    let res = app.oneshot(get_with_bearer(&next, &token)).await.unwrap();
    let json = body_json(res).await;
    assert_eq!(json["publications"].as_array().unwrap().len(), 5);
    assert!(!json["links"]
        .as_array()
        .unwrap()
        .iter()
        .any(|l| l["rel"] == "next"));
}

#[tokio::test]
async fn all_books_rejects_a_malformed_cursor() {
    let (app, _pool, token) = fixture().await;
    for uri in [
        "/opds/all?cursor=%%%not-a-cursor",
        "/opds/v2/all?cursor=%%%not-a-cursor",
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(uri, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "uri={uri}");
    }
}

#[tokio::test]
async fn atom_series_browse_matches_the_json_catalog() {
    // AC3 (#1812): the Atom series browse serves the same series and
    // members as the /opds/v2 equivalent it reaches parity with.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook_with_series(
        &pool,
        "dune.epub",
        "Dune",
        "Frank Herbert",
        &["Science Fiction"],
        ("Dune Saga", "1"),
    )
    .await;
    let series_id = series_id_by_name(&pool, "Dune Saga").await;

    let res = app
        .clone()
        .oneshot(get_with_bearer("/opds/series", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains("<title>Dune Saga</title>"));
    assert!(body.contains(&format!("href=\"/opds/series/{series_id}\"")));

    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/opds/series/{series_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let atom_body = body_string(res).await;
    let atom_count = atom_body.matches("<entry>").count();
    assert!(atom_body.contains("<title>Dune</title>"));

    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/opds/v2/series/{series_id}"),
            &token,
        ))
        .await
        .unwrap();
    let json = body_json(res).await;
    assert_eq!(atom_count, json["publications"].as_array().unwrap().len());

    let res = app
        .oneshot(get_with_bearer("/opds/series/999999", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

// ---------------------------------------------------------------- Shelves

async fn seed_shelf(
    pool: &SqlitePool,
    owner_id: i64,
    name: &str,
    visibility: omnibus_shared::Visibility,
    book_uuids: Vec<String>,
) -> omnibus_shared::Shelf {
    db::create_shelf(
        pool,
        owner_id,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: name.into(),
            description: None,
            visibility,
            match_mode: None,
            rules: Vec::new(),
            book_uuids,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn shelves_browse_lists_only_the_viewers_visible_shelves() {
    // AC1 (#1813): the listing is the viewer's — owner sees their private
    // shelf, another user sees only the public one; identically in both
    // catalogs (AC3).
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_shelf(
        &pool,
        owner.id,
        "Secret Stack",
        omnibus_shared::Visibility::Private,
        vec![uuid.clone()],
    )
    .await;
    seed_shelf(
        &pool,
        owner.id,
        "Front Window",
        omnibus_shared::Visibility::Public,
        vec![uuid],
    )
    .await;

    for (tok, sees_private, who) in [(&owner_token, true, "owner"), (&token, false, "other")] {
        for base in ["/opds/shelves", "/opds/v2/shelves"] {
            let res = app
                .clone()
                .oneshot(get_with_bearer(base, tok))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{who} {base}");
            let body = body_string(res).await;
            assert!(body.contains("Front Window"), "{who} {base}");
            assert_eq!(body.contains("Secret Stack"), sees_private, "{who} {base}");
        }
    }
}

#[tokio::test]
async fn private_shelf_feed_404s_for_a_non_viewer_and_serves_members_for_the_owner() {
    // AC2 (#1813): the per-shelf feed serves members to a permitted viewer
    // and 404s (not 403 — no existence leak) for anyone else.
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let shelf = seed_shelf(
        &pool,
        owner.id,
        "Secret Stack",
        omnibus_shared::Visibility::Private,
        vec![uuid],
    )
    .await;

    for base in ["/opds/shelves", "/opds/v2/shelves"] {
        let uri = format!("{base}/{}", shelf.id);
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &owner_token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "owner {uri}");
        let body = body_string(res).await;
        assert!(body.contains("Dune"), "owner {uri}");

        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND, "other {uri}");
    }

    // Unknown id 404s the same way for everyone, in both catalogs.
    for base in ["/opds/shelves", "/opds/v2/shelves"] {
        for tok in [&owner_token, &token] {
            let res = app
                .clone()
                .oneshot(get_with_bearer(&format!("{base}/999999"), tok))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::NOT_FOUND, "{base}");
        }
    }
}

#[tokio::test]
async fn shelf_feeds_exclude_non_ebook_library_members() {
    // The opds module's ebook-library invariant, for shelves: `shelf_page`
    // is not path-scoped, so a mixed shelf can hold audiobook-library
    // members — the e-reader-format filter must drop them from the feed.
    let (app, pool, token) = fixture().await;
    let user = auth_test_support::create_user(&pool, "mixed-owner").await;
    let user_token = auth_test_support::bearer_token(&pool, user.id).await;
    let epub = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let audio = db::test_support::seed_synced_audiobook(
        &pool,
        "hail-mary",
        "Project Hail Mary",
        "Andy Weir",
    )
    .await;
    let shelf = seed_shelf(
        &pool,
        user.id,
        "Mixed Media",
        omnibus_shared::Visibility::Public,
        vec![epub, audio],
    )
    .await;

    for base in ["/opds/shelves", "/opds/v2/shelves"] {
        let uri = format!("{base}/{}", shelf.id);
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &user_token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{uri}");
        let body = body_string(res).await;
        assert!(body.contains("Dune"), "{uri}");
        assert!(
            !body.contains("Project Hail Mary"),
            "{uri} must drop the audiobook-library member"
        );
    }
    let _ = token;
}
