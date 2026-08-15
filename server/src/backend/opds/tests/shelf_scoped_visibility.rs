//! Shelf-scoped feed visibility (#932): a book confined to a manual shelf
//! the viewer cannot see must not surface in a browse/search/new/nav feed,
//! or the byte-serving delegate routes, for anyone but the shelf's owner,
//! a public-shelf viewer, or an admin — in both catalogs.

use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::get_with_bearer;

use super::{
    author_id_by_name, body_string, fixture, seed_synced_ebook_with_series, series_id_by_name,
};

/// Seed one book with a *real* on-disk EPUB file — unlike `seed_synced_ebook`,
/// which writes DB rows only — so a 200-vs-404 split in a delegate-route test
/// proves the shelf gate rather than a missing-file 404 either way. Returns
/// the `tempfile::TempDir` guard; keep it alive for the file's lifetime.
async fn seed_downloadable_book(
    pool: &sqlx::SqlitePool,
    uuid: &str,
    title: &str,
) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("alpha.epub"), b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.path().to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?)")
            .bind(uuid)
            .bind(lib_id)
            .bind(tmp.path().to_str().unwrap())
            .bind(title)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    tmp
}

async fn seed_private_manual_shelf(
    pool: &sqlx::SqlitePool,
    owner_id: i64,
    book_uuid: String,
) -> omnibus_shared::Shelf {
    db::create_shelf(
        pool,
        owner_id,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Secret Stack".into(),
            description: None,
            visibility: omnibus_shared::Visibility::Private,
            match_mode: None,
            rules: Vec::new(),
            book_uuids: vec![book_uuid],
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn all_books_excludes_a_shelf_exclusive_book_for_a_non_owner_and_includes_it_for_the_owner_and_admin(
) {
    // AC2 (#932): a book confined to a private shelf must not appear in
    // "All Books" for anyone but the owner or an admin — identically in
    // both catalogs (AC3).
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let admin = auth_test_support::create_admin(&pool, "shelf-admin").await;
    let admin_token = auth_test_support::bearer_token(&pool, admin.id).await;

    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_private_manual_shelf(&pool, owner.id, uuid).await;

    for (tok, sees_it, who) in [
        (&owner_token, true, "owner"),
        (&admin_token, true, "admin"),
        (&token, false, "other"),
    ] {
        for base in ["/opds/all", "/opds/v2/all"] {
            let res = app
                .clone()
                .oneshot(get_with_bearer(base, tok))
                .await
                .unwrap();
            assert_eq!(res.status(), StatusCode::OK, "{who} {base}");
            let body = body_string(res).await;
            assert_eq!(body.contains("Dune"), sees_it, "{who} {base}");
        }
    }
}

#[tokio::test]
async fn a_book_on_no_shelf_is_unaffected_by_shelf_visibility_filtering() {
    // Guard against over-filtering: the overwhelming majority of library
    // books are on no shelf at all and must show for every viewer exactly
    // as before.
    let (app, pool, token) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;

    for base in ["/opds/all", "/opds/v2/all"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{base}");
        assert!(body_string(res).await.contains("Dune"), "{base}");
    }
}

#[tokio::test]
async fn a_book_on_a_public_shelf_shows_normally_for_every_viewer() {
    // AC1's clarification: a book reachable through a *visible* shelf is
    // not shelf-exclusive and must show normally.
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    db::create_shelf(
        &pool,
        owner.id,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Front Window".into(),
            description: None,
            visibility: omnibus_shared::Visibility::Public,
            match_mode: None,
            rules: Vec::new(),
            book_uuids: vec![uuid],
        },
    )
    .await
    .unwrap();

    for base in ["/opds/all", "/opds/v2/all"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{base}");
        assert!(body_string(res).await.contains("Dune"), "{base}");
    }
}

#[tokio::test]
async fn new_arrivals_excludes_a_shelf_exclusive_book_for_a_non_owner() {
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_private_manual_shelf(&pool, owner.id, uuid).await;

    for base in ["/opds/new", "/opds/v2/new"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &token))
            .await
            .unwrap();
        assert!(!body_string(res).await.contains("Dune"), "other {base}");

        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &owner_token))
            .await
            .unwrap();
        assert!(body_string(res).await.contains("Dune"), "owner {base}");
    }
}

#[tokio::test]
async fn search_excludes_a_shelf_exclusive_book_for_a_non_owner() {
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_private_manual_shelf(&pool, owner.id, uuid).await;

    for base in ["/opds/search?q=dune", "/opds/v2/search?q=dune"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &token))
            .await
            .unwrap();
        assert!(!body_string(res).await.contains("Dune"), "other {base}");

        let res = app
            .clone()
            .oneshot(get_with_bearer(base, &owner_token))
            .await
            .unwrap();
        assert!(body_string(res).await.contains("Dune"), "owner {base}");
    }
}

#[tokio::test]
async fn author_acquisition_feed_excludes_a_shelf_exclusive_book_for_a_non_owner() {
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    seed_private_manual_shelf(&pool, owner.id, uuid).await;
    let author_id = author_id_by_name(&pool, "Frank Herbert").await;

    for base in [
        format!("/opds/author/{author_id}"),
        format!("/opds/v2/author/{author_id}"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&base, &token))
            .await
            .unwrap();
        assert!(!body_string(res).await.contains("Dune"), "other {base}");

        let res = app
            .clone()
            .oneshot(get_with_bearer(&base, &owner_token))
            .await
            .unwrap();
        assert!(body_string(res).await.contains("Dune"), "owner {base}");
    }
}

#[tokio::test]
async fn series_acquisition_feed_excludes_a_shelf_exclusive_book_for_a_non_owner() {
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;
    seed_synced_ebook_with_series(
        &pool,
        "dune.epub",
        "Dune",
        "Frank Herbert",
        &[],
        ("Dune Saga", "1"),
    )
    .await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE title = 'Dune'")
        .fetch_one(&pool)
        .await
        .unwrap();
    seed_private_manual_shelf(&pool, owner.id, uuid.clone()).await;
    let series_id = series_id_by_name(&pool, "Dune Saga").await;

    // Match on the book's own acquisition link (embeds its uuid), not the
    // string "Dune" — the series feed's own <title> is "Dune Saga" and
    // would false-positive a naive substring check.
    let needle = format!("/opds/ebooks/{uuid}/");
    for base in [
        format!("/opds/series/{series_id}"),
        format!("/opds/v2/series/{series_id}"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&base, &token))
            .await
            .unwrap();
        assert!(!body_string(res).await.contains(&needle), "other {base}");

        let res = app
            .clone()
            .oneshot(get_with_bearer(&base, &owner_token))
            .await
            .unwrap();
        assert!(body_string(res).await.contains(&needle), "owner {base}");
    }
}

#[tokio::test]
async fn delegate_routes_404_a_shelf_exclusive_book_for_a_non_owner_and_serve_it_for_the_owner() {
    // The byte-serving routes must agree with the feeds: a book that never
    // surfaces in a listing must not be fetchable by uuid either.
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;
    let owner_token = auth_test_support::bearer_token(&pool, owner.id).await;

    let uuid = "55555555-5555-5555-5555-555555555555";
    let _tmp = seed_downloadable_book(&pool, uuid, "Alpha").await;
    seed_private_manual_shelf(&pool, owner.id, uuid.to_string()).await;

    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/opds/ebooks/{uuid}/file"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND, "other must 404");

    let res = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/opds/ebooks/{uuid}/file"),
            &owner_token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK, "owner must still fetch it");
}

#[tokio::test]
async fn delegate_routes_404_a_shelf_hidden_book_requested_by_its_old_merged_uuid() {
    // Regression (#932 review): the media handlers resolve a path uuid via
    // `resolve_book_id_by_uuid`, which falls back to `merged_uuids` — a book
    // merged away from an old uuid still serves under it. The shelf gate
    // must canonicalize the same way, or an old/merged uuid walks straight
    // past it and serves a book that's supposed to be hidden.
    let (app, pool, token) = fixture().await;
    let owner = auth_test_support::create_user(&pool, "shelf-owner").await;

    let canonical_uuid = "66666666-6666-6666-6666-666666666666";
    let _tmp = seed_downloadable_book(&pool, canonical_uuid, "Beta").await;
    seed_private_manual_shelf(&pool, owner.id, canonical_uuid.to_string()).await;

    // An old uuid that was merged away, now resolving to the same book —
    // written directly rather than via `merge_books`, since only the
    // `merged_uuids` fallback (not the merge transaction itself) is under
    // test here.
    let old_uuid = "77777777-7777-7777-7777-777777777777";
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(canonical_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path) \
         VALUES (?, ?, 'EPUB', '/lib')",
    )
    .bind(old_uuid)
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    // Both routes actually resolve the old uuid to a real, servable file via
    // `merged_uuids` — so a 200 here (rather than a coincidental 404 for
    // some other reason) is exactly what the bypass would have produced.
    for uri in [
        format!("/opds/ebooks/{old_uuid}/file"),
        format!("/opds/ebooks/{old_uuid}/download"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &token))
            .await
            .unwrap();
        assert_eq!(
            res.status(),
            StatusCode::NOT_FOUND,
            "old uuid must not bypass the shelf gate via merged_uuids: {uri}"
        );
    }
}
