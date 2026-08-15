//! HTTP-layer contract tests for `/opds/*`, driving `opds_router` directly
//! (like `kobo/tests.rs`) via `oneshot` against an in-memory DB. Shared
//! fixtures live here, alongside the Atom v1 catalog and timestamp-parsing
//! tests; the JSON catalog, HTTP Basic auth, All Books/Series/Shelves, and
//! shelf-scoped visibility tests are split into the sibling modules below.

mod all_series_shelves;
mod basic_auth;
mod json_catalog;
mod shelf_scoped_visibility;

use axum::{
    body::to_bytes,
    extract::{Path, Query, State},
    http::StatusCode,
    Router,
};
use omnibus_db::{self as db, auth::SessionKind, test_support::seed_synced_ebook};
use omnibus_shared::Settings;
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::auth::{AuthUser, OpdsAuthUser};
use crate::backend::test_support::{get_anon, get_with_bearer, CoversDirGuard, TINY_PNG};

use super::*;

/// Minimal `OpdsAuthUser` for driving a handler directly, bypassing the
/// extractor — mirroring `fake_admin` in `admin_sessions/tests.rs`. Needed
/// because each handler's own DB-failure branch shares the `sessions`
/// table with `OpdsAuthUser`'s own session lookup, so closing the pool
/// before a routed request would fail extraction itself, not the handler
/// body under test.
fn fake_opds_user() -> OpdsAuthUser {
    OpdsAuthUser(AuthUser {
        id: 1,
        username: "opds-reader".to_string(),
        is_admin: false,
        can_upload: false,
        can_edit: false,
        can_download: true,
        kindle_email: None,
        display_name: None,
        has_avatar: false,
        hidden_formats: Vec::new(),
        session_id: 1,
        session_kind: SessionKind::Bearer,
    })
}

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

/// Seed one ebook (like `seed_synced_ebook`) with subjects and a series too,
/// so the series-browse and category-parity tests have something to
/// browse/assert on. Shared by the JSON-catalog and All/Series/Shelves
/// suites — no Atom-only test needs it.
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
async fn search_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = search::search(
        fake_opds_user(),
        State(state),
        Query(search::SearchQuery { q: "dune".into() }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
async fn new_arrivals_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = new::new_arrivals(fake_opds_user(), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
async fn authors_letter_index_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = authors::letter_index(fake_opds_user(), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
async fn authors_by_letter_rejects_a_single_non_alphabetic_non_hash_character() {
    let (app, _pool, token) = fixture().await;
    for uri in ["/opds/authors/1", "/opds/authors/!"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(uri, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "uri={uri}");
    }
}

#[tokio::test]
async fn authors_by_letter_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = authors::by_letter(fake_opds_user(), State(state), Path("F".into())).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
async fn author_acquisition_feed_cover_link_reflects_a_non_jpeg_cover_format() {
    // #1772: the cover `<link>`'s `type` must match the actual on-disk
    // format rather than a hardcoded `image/jpeg` literal.
    let (app, pool, token) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("opds_atom_png_cover");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG).unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/opds/author/{author_id}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_string(res).await;
    assert!(body.contains(&format!(r#"href="/opds/covers/{uuid}" type="image/png""#)));
    assert!(!body.contains(&format!(r#"href="/opds/covers/{uuid}" type="image/jpeg""#)));
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

#[tokio::test]
async fn author_acquisition_feed_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = authors::acquisition_feed(fake_opds_user(), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// --------------------------------------------------------- Timestamp parsing

#[tokio::test]
async fn entry_updated_falls_back_to_the_current_instant_for_a_missing_timestamp() {
    let before = time::OffsetDateTime::now_utc() - time::Duration::seconds(2);
    let result = entry_updated(None);
    let after = time::OffsetDateTime::now_utc() + time::Duration::seconds(2);
    let parsed =
        time::OffsetDateTime::parse(&result, &time::format_description::well_known::Rfc3339)
            .unwrap();
    assert!(
        parsed >= before && parsed <= after,
        "expected {parsed} between {before} and {after}"
    );
}

#[tokio::test]
async fn entry_updated_falls_back_to_the_current_instant_for_a_malformed_timestamp() {
    let before = time::OffsetDateTime::now_utc() - time::Duration::seconds(2);
    let result = entry_updated(Some("not-a-timestamp"));
    let after = time::OffsetDateTime::now_utc() + time::Duration::seconds(2);
    let parsed =
        time::OffsetDateTime::parse(&result, &time::format_description::well_known::Rfc3339)
            .unwrap();
    assert!(
        parsed >= before && parsed <= after,
        "expected {parsed} between {before} and {after}"
    );
}
