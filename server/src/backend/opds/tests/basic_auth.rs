//! HTTP Basic auth for `/opds/*`: the fallback OPDS clients like KOReader
//! rely on, its rate limiting and lockout behaviour, the byte-serving
//! delegates it unlocks, and the ebook-library physical-only exclusion that
//! shares this fixture set.

use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::get_with_bearer;

use super::super::*;
use super::{author_id_by_name, body_string, fixture};

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
async fn basic_auth_returns_500_when_the_pool_closes_during_credential_verification() {
    // A closed pool must surface as a 5xx during `verify_login`, not
    // silently fall through to an anonymous 401 challenge.
    let (app, pool, _token) = fixture().await;
    pool.close().await;
    let res = app
        .oneshot(get_with_basic("/opds", "basic-reader", "opds-basic-pass-1"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
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
