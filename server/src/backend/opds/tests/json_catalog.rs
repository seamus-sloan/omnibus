//! `/opds/v2/*` — the OPDS 2.0 JSON catalog's contract tests: auth parity
//! with the Atom feeds, and the per-publication shape (links, images,
//! series position, categories) the JSON format carries that Atom doesn't.

use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use tower::ServiceExt;

use crate::backend::test_support::{get_anon, get_with_bearer, CoversDirGuard, TINY_PNG};

use super::super::*;
use super::{
    author_id_by_name, body_json, body_string, fixture, seed_ebook_in_library,
    seed_synced_ebook_with_series, series_id_by_name,
};

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
async fn v2_author_acquisition_feed_cover_link_reflects_a_non_jpeg_cover_format() {
    // #1772, JSON counterpart: `book_publication`'s image `type` must match
    // `entries::book_entry`'s, keeping the two catalogs in agreement (AC3).
    let (app, pool, token) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("opds_v2_png_cover");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG).unwrap();

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
    let images = pubs[0]["images"].as_array().unwrap();
    assert!(images
        .iter()
        .any(|l| l["rel"] == "http://opds-spec.org/image" && l["type"] == "image/png"));
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
