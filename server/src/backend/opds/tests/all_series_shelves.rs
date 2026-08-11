//! `/opds/all`, `/opds/series*`, and `/opds/shelves*` (plus their `/v2`
//! JSON counterparts): keyset pagination across the whole library, the
//! series browse's parity with the JSON catalog, and shelf visibility —
//! the one OPDS surface that reads the caller's identity.

use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::get_with_bearer;

use super::super::*;
use super::{
    body_json, body_string, fake_opds_user, fixture, seed_synced_ebook_with_series,
    series_id_by_name,
};

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
async fn all_books_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = all::all_books(
        fake_opds_user(),
        State(state),
        Query(all::AllQuery { cursor: None }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn v2_all_books_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = json_all::all_books(
        fake_opds_user(),
        State(state),
        Query(all::AllQuery { cursor: None }),
    )
    .await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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

#[tokio::test]
async fn series_index_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = series::index(fake_opds_user(), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn series_acquisition_feed_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = series::acquisition_feed(fake_opds_user(), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

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

#[tokio::test]
async fn shelves_index_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = shelves::index(fake_opds_user(), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn shelves_acquisition_feed_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = shelves::acquisition_feed(fake_opds_user(), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn v2_shelves_index_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = json_shelves::index(fake_opds_user(), State(state)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn v2_shelves_acquisition_feed_returns_500_when_the_pool_is_closed() {
    let (_app, pool, _token) = fixture().await;
    let state = AppState::new(pool.clone());
    pool.close().await;
    let res = json_shelves::acquisition_feed(fake_opds_user(), State(state), Path(1)).await;
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
