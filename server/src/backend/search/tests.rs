//! Tests for search and command-palette handlers.
use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;
use crate::backend::SEARCH_RATE_LIMIT_MAX;

async fn configure_and_seed_audiobook(pool: &sqlx::SqlitePool) {
    db::set_settings(
        pool,
        &Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: Some("/audiobooks".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    db::sync_audiobooks(
        pool,
        "/audiobooks",
        db::sync::AudiobookSyncPlan {
            new_books: vec![db::audiobook::IndexedAudiobook {
                scan_key: "Le Guin/Earthsea.m4b".into(),
                group_path: "Le Guin/Earthsea.m4b".into(),
                format: "M4B".into(),
                title: "A Wizard of Earthsea".into(),
                creator_name: Some("Ursula K. Le Guin".into()),
                cover: None,
                accent: None,
                parts: vec![db::audiobook::AudiobookPart {
                    ordinal: 0,
                    filename: "Le Guin/Earthsea.m4b".into(),
                    size_bytes: 1,
                    mtime_epoch: 1,
                    duration_seconds: 1.0,
                }],
                chapters: vec![],
                total_size_bytes: 1,
                max_mtime_epoch: 1,
                description: None,
                error: None,
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn api_get_search_sets_total_count_header_with_indexed_library() {
    // Issue #81: /api/search must attach X-Total-Count on every response,
    // matching the /api/ebooks contract.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    db::replace_books(
        &pool,
        "/lib",
        vec![
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
                word_count: None,
            },
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "beta.epub".into(),
                    title: Some("Beta".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
                word_count: None,
            },
        ],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(get_with_bearer("/api/search?q=Alpha", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "X-Total-Count must reflect the FTS match count"
    );
    assert!(
        response.headers().get("X-Total-Cap").is_none(),
        "X-Total-Cap must not be set when search results fit under the cap"
    );
}

#[tokio::test]
async fn api_get_search_returns_matching_audiobook_from_configured_audiobook_library() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    configure_and_seed_audiobook(&pool).await;

    let response = app
        .oneshot(get_with_bearer("/api/search?q=Earthsea", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let library: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(library.books.len(), 1);
    assert_eq!(
        library.books[0].title.as_deref(),
        Some("A Wizard of Earthsea")
    );
}

#[tokio::test]
async fn api_get_search_sets_total_count_zero_when_path_not_configured() {
    // Issue #81: the early-return path (no library configured) must
    // still attach X-Total-Count: 0 so the client can rely on the
    // header always being present.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/search?q=anything", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("0"),
        "X-Total-Count must be 0 on the no-library-configured early return"
    );
    assert!(
        response.headers().get("X-Total-Cap").is_none(),
        "X-Total-Cap must not be set on the early-return path"
    );
}

#[tokio::test]
async fn api_search_returns_empty_when_path_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert!(lib.path.is_none());
    assert!(lib.books.is_empty());
}

#[tokio::test]
async fn api_search_rejects_missing_q_param() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search", &token))
        .await
        .expect("request should succeed");
    // axum's Query extractor returns 400 for missing required fields.
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_rejects_over_length_q_with_400() {
    // Issue #638: the handler caps the decoded `q` length at
    // MAX_SEARCH_QUERY_LEN bytes and rejects longer input with 400 before any
    // search db call.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized = "a".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search?q={oversized}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/search?q=hello")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// -------------------------------------------------------------------
// /api/search/palette — search palette (F1.5)
// -------------------------------------------------------------------

#[tokio::test]
async fn api_search_palette_returns_matching_audiobook_from_configured_audiobook_library() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    configure_and_seed_audiobook(&pool).await;

    let response = app
        .oneshot(get_with_bearer("/api/search/palette?q=Earthsea", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::PaletteResults = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(results.books.len(), 1);
    assert_eq!(results.books[0].title, "A Wizard of Earthsea");
    assert_eq!(results.books[0].formats, ["M4B"]);
}

/// Issue #1788: a checked-in print book lives under the synthetic
/// `physical://local` root, which is never a configured library path. The
/// palette scoped on the path alone, so `/api/search` returned the book and
/// `/api/search/palette` did not.
#[tokio::test]
async fn api_search_palette_returns_a_physical_only_book_with_a_copy() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let uuid = db::create_fileless_book(
        &pool,
        db::FilelessBook {
            title: "Paper Only".into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    db::add_physical_copy(&pool, &uuid, None, None, None)
        .await
        .unwrap();

    // The books arm matches on title, the authors arm on the author name —
    // two queries, because no token is shared between them.
    let by_title: omnibus_shared::PaletteResults =
        palette_query(app.clone(), &token, "paper").await;
    assert_eq!(by_title.books.len(), 1, "got {by_title:?}");
    assert_eq!(by_title.books[0].title, "Paper Only");

    let by_author: omnibus_shared::PaletteResults = palette_query(app, &token, "lovelace").await;
    assert_eq!(by_author.authors.len(), 1, "got {by_author:?}");
    assert_eq!(by_author.authors[0].name, "Ada Lovelace");
    assert_eq!(by_author.authors[0].book_count, 1);
}

#[tokio::test]
async fn api_search_palette_carries_a_genres_group_with_its_uncapped_total() {
    // The widened payload is what the web palette and `/search` render the
    // Genres group from, and what the native iOS client will read once it
    // grows one — assert it on the wire, not just in the db layer.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Dark Water").await;
    db::upsert_metadata_overrides(
        &pool,
        &uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(vec!["Gothic Horror".into()]),
            ..Default::default()
        },
        false,
        user.id,
    )
    .await
    .unwrap();

    let results: omnibus_shared::PaletteResults = palette_query(app, &token, "gothic").await;
    assert_eq!(results.genres.len(), 1, "got {results:?}");
    assert_eq!(results.genres[0].name, "Gothic Horror");
    assert_eq!(results.genres[0].book_count, 1);
    assert_eq!(results.genre_total, 1);
    assert!(
        results.total_count() >= 1,
        "genre_total must feed total_count"
    );
}

/// Issue one palette request and decode the body, asserting a 200 on the way.
async fn palette_query(app: axum::Router, token: &str, q: &str) -> omnibus_shared::PaletteResults {
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search/palette?q={q}"),
            token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_search_palette_returns_empty_when_path_not_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::PaletteResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.books.is_empty());
    assert!(results.authors.is_empty());
}

#[tokio::test]
async fn api_search_palette_rejects_over_length_q_with_400() {
    // Issue #638: the palette handler shares the same MAX_SEARCH_QUERY_LEN
    // cap, rejecting over-length input with 400 before any search db call.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized = "a".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search/palette?q={oversized}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_search_palette_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/search/palette?q=hello"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_search_palette_returns_429_after_budget_exceeded() {
    // Issue #124: /api/search/* runs four heavy FTS5 queries per request,
    // so it gets a per-IP fixed-window rate limit. The limit is set to
    // SEARCH_RATE_LIMIT_MAX requests per SEARCH_RATE_LIMIT_WINDOW; the
    // (SEARCH_RATE_LIMIT_MAX + 1)th request from the same principal must
    // be rejected with 429.
    //
    // `oneshot` requests carry no `ConnectInfo<SocketAddr>` extension, so
    // the limiter's IP fallback (`0.0.0.0`) applies — every request in
    // this test shares one bucket, which is exactly what we want.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for i in 0..SEARCH_RATE_LIMIT_MAX {
        let res = app
            .clone()
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "request #{} should be within budget",
            i + 1
        );
    }

    // The (MAX+1)th request must trip the limiter.
    let over_limit = app
        .clone()
        .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(
        over_limit.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "request beyond SEARCH_RATE_LIMIT_MAX must return 429",
    );
}
