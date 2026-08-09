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

async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn every_opds_route_401s_without_a_session() {
    // AC4: every /opds route requires authentication.
    let (app, _pool, _token) = fixture().await;
    for uri in [
        "/opds",
        "/opds/osd",
        "/opds/search?q=x",
        "/opds/new",
        "/opds/authors",
        "/opds/authors/A",
        "/opds/author/1",
    ] {
        let res = app.clone().oneshot(get_anon(uri)).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "uri={uri}");
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
