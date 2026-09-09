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

// -------------------------------------------------------------------
// /api/search/content — book-content search (#2282)
// -------------------------------------------------------------------

/// Seed one indexed book plus a `book_content_chapters` row carrying `text`,
/// returning the book's uuid. Raw insert rather than the worker pass — the
/// extraction pipeline is covered by the db-crate tests; here only the wire
/// contract is under test.
async fn seed_content_chapter(pool: &sqlx::SqlitePool, text: &str) -> String {
    db::set_settings(
        pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    db::replace_books(
        pool,
        "/lib",
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: "alpha.epub".into(),
                title: Some("Alpha".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 7,
            size_bytes: 9,
            word_count: None,
        }],
    )
    .await
    .unwrap();
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_content_chapters (book_uuid, spine_index, mtime_epoch, size_bytes, text) \
         VALUES (?, 2, 7, 9, ?)",
    )
    .bind(&uuid)
    .bind(text)
    .execute(pool)
    .await
    .unwrap();
    uuid
}

#[tokio::test]
async fn api_get_search_content_returns_hits_with_chapter_citation() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;

    let response = app
        .oneshot(get_with_bearer("/api/search/content?q=lighthouse", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].book_uuid, uuid);
    assert_eq!(results.hits[0].spine_index, 2);
    assert_eq!(results.hits[0].title, "Alpha");
    assert!(
        results.hits[0].snippet.contains("[lighthouse]"),
        "snippet must mark the matched term: {}",
        results.hits[0].snippet
    );
}

#[tokio::test]
async fn api_get_search_content_returns_empty_hits_when_no_library_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/search/content?q=anything", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.hits.is_empty());
}

#[tokio::test]
async fn api_get_search_content_rejects_over_length_q_with_400() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let oversized = "a".repeat(MAX_SEARCH_QUERY_LEN + 1);
    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/search/content?q={oversized}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_search_content_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/search/content?q=hello"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_search_content_returns_500_when_the_db_is_gone() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // A configured library path gets the handler past its unconfigured
    // early-out and into the FTS query the drop below breaks.
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
    sqlx::query("DROP TABLE book_content_fts")
        .execute(&pool)
        .await
        .unwrap();
    let res = app
        .oneshot(get_with_bearer("/api/search/content?q=hello", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_get_search_content_returns_429_after_budget_exceeded() {
    // The content route is registered inside `search_router`, so it shares
    // the per-IP search rate limit with `/api/search` and the palette.
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    for i in 0..SEARCH_RATE_LIMIT_MAX {
        let res = app
            .clone()
            .oneshot(get_with_bearer("/api/search/content?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(
            res.status(),
            StatusCode::OK,
            "request #{} should be within budget",
            i + 1
        );
    }

    let over_limit = app
        .clone()
        .oneshot(get_with_bearer("/api/search/content?q=hello", &token))
        .await
        .expect("request should succeed");
    assert_eq!(
        over_limit.status(),
        StatusCode::TOO_MANY_REQUESTS,
        "request beyond SEARCH_RATE_LIMIT_MAX must return 429",
    );
}

/// Add a second content chapter to an existing book, so a scope/limit test
/// has more than one row to narrow.
async fn seed_extra_chapter(pool: &sqlx::SqlitePool, uuid: &str, spine_index: i64, text: &str) {
    sqlx::query(
        "INSERT INTO book_content_chapters (book_uuid, spine_index, mtime_epoch, size_bytes, text) \
         VALUES (?, ?, 7, 9, ?)",
    )
    .bind(uuid)
    .bind(spine_index)
    .bind(text)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn api_get_search_content_scopes_to_one_book_and_honours_limit() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;
    seed_extra_chapter(&pool, &uuid, 4, "Another lighthouse, another shore.").await;

    let response = app
        .clone()
        .oneshot(get_with_bearer(
            &format!("/api/search/content?q=lighthouse&book_uuid={uuid}&limit=1"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(results.hits.len(), 1, "limit caps the hit list");
    assert_eq!(results.hits[0].book_uuid, uuid);

    // A uuid that matches nothing scopes the search to nothing, rather than
    // being ignored and searching the whole library.
    let response = app
        .oneshot(get_with_bearer(
            "/api/search/content?q=lighthouse&book_uuid=no-such-book",
            &token,
        ))
        .await
        .expect("request should succeed");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.hits.is_empty());
}

#[tokio::test]
async fn api_get_search_content_hints_when_a_multi_term_query_matches_nothing() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;

    // Both words are in the book, in different chapters — the index ANDs
    // them, so the phrase matches nothing and the caller needs telling why.
    seed_extra_chapter(&pool, &sole_uuid(&pool).await, 5, "A whiskey at dusk.").await;
    let response = app
        .oneshot(get_with_bearer(
            "/api/search/content?q=lighthouse%20whiskey",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.hits.is_empty());
    let hint = results
        .hint
        .expect("an empty multi-term search explains itself");
    assert!(
        hint.contains("lighthouse") && hint.contains("whiskey"),
        "the hint names the terms that matched on their own: {hint}"
    );
}

#[tokio::test]
async fn api_get_search_content_gives_no_hint_for_a_single_term_that_is_simply_absent() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;

    let response = app
        .oneshot(get_with_bearer("/api/search/content?q=pangolin", &token))
        .await
        .expect("request should succeed");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.hits.is_empty());
    assert!(
        results.hint.is_none(),
        "one term cannot be an AND problem; a hint here would misdirect"
    );
}

#[tokio::test]
async fn api_get_search_content_excludes_withhold_hits_it_cannot_place() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;

    // No reading position and no extracted spine stats: the hit cannot be
    // shown to be behind the reader, so `exclude` must withhold it rather
    // than treat "can't tell" as "safe".
    let response = app
        .oneshot(get_with_bearer(
            "/api/search/content?q=lighthouse&spoiler_filter=exclude",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert!(results.hits.is_empty());
    assert_eq!(
        results.withheld_ahead,
        Some(1),
        "the count is how a caller says 'there is an answer ahead of you'"
    );
}

#[tokio::test]
async fn api_get_search_content_leaves_hits_unmarked_under_spoiler_filter_none() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;

    let response = app
        .oneshot(get_with_bearer(
            "/api/search/content?q=lighthouse&spoiler_filter=none",
            &token,
        ))
        .await
        .expect("request should succeed");
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let results: omnibus_shared::ContentSearchResults = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(results.hits.len(), 1);
    assert!(results.hits[0].ahead_of_reader.is_none());
    assert!(results.withheld_ahead.is_none());
}

/// Give the seeded book the spine stats the spoiler boundary measures
/// against: four equal documents, so spine `n` begins at `25 * n` percent.
/// Written directly rather than extracted from an archive — the boundary
/// reads these rows, not the file.
async fn seed_spine_stats(pool: &sqlx::SqlitePool, uuid: &str) {
    let book_id = db::resolve_book_id_by_uuid(pool, uuid)
        .await
        .unwrap()
        .unwrap();
    let (file_id, _) = db::book_file_with_id(pool, book_id, "EPUB")
        .await
        .unwrap()
        .expect("the seeded book has an EPUB file");
    for spine_index in 0..4i64 {
        sqlx::query(
            "INSERT INTO epub_spine_stats (book_file_id, spine_index, href, visible_chars, chars_before)
             VALUES (?, ?, ?, 1000, ?)",
        )
        .bind(file_id)
        .bind(spine_index)
        .bind(format!("c{spine_index}.xhtml"))
        .bind(spine_index * 1000)
        .execute(pool)
        .await
        .unwrap();
    }
}

/// Put the reader at a whole-book percent, the figure the boundary compares
/// each hit against.
async fn save_reader_percent(pool: &sqlx::SqlitePool, user_id: i64, uuid: &str, percent: i64) {
    db::progress::upsert_progress(
        pool,
        user_id,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(percent),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1),
        },
    )
    .await
    .unwrap();
}

/// Seed one book whose only indexed text sits at spine 2 — the halfway mark —
/// and put the reader at `reader_percent`.
async fn fixture_with_reader_at(reader_percent: i64) -> (axum::Router, sqlx::SqlitePool, String) {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_content_chapter(&pool, "The lighthouse keeper counted the waves.").await;
    seed_spine_stats(&pool, &uuid).await;
    save_reader_percent(&pool, user.id, &uuid, reader_percent).await;
    (app, pool, token)
}

async fn content_search(
    app: axum::Router,
    query: &str,
    token: &str,
) -> omnibus_shared::ContentSearchResults {
    let response = app
        .oneshot(get_with_bearer(query, token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_get_search_content_annotates_a_hit_ahead_of_the_reader_with_its_distance() {
    // Reader at 30%, the only hit at the 50% mark: ahead, by 20 points.
    let (app, _pool, token) = fixture_with_reader_at(30).await;

    let results = content_search(
        app,
        "/api/search/content?q=lighthouse&spoiler_filter=annotate",
        &token,
    )
    .await;

    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].ahead_of_reader, Some(true));
    assert_eq!(results.hits[0].position_delta_percent, Some(20.0));
    assert!(
        results.withheld_ahead.is_none(),
        "annotate reports the distance; it withholds nothing"
    );
}

#[tokio::test]
async fn api_get_search_content_annotates_a_hit_the_reader_has_passed_as_behind_them() {
    // Reader at 60%, past the 50% mark the hit sits at: behind, by 10 points.
    let (app, _pool, token) = fixture_with_reader_at(60).await;

    let results = content_search(
        app,
        "/api/search/content?q=lighthouse&spoiler_filter=annotate",
        &token,
    )
    .await;

    assert_eq!(results.hits.len(), 1);
    assert_eq!(results.hits[0].ahead_of_reader, Some(false));
    assert_eq!(results.hits[0].position_delta_percent, Some(-10.0));
}

#[tokio::test]
async fn api_get_search_content_excludes_a_hit_ahead_of_the_reader_and_counts_it() {
    let (app, _pool, token) = fixture_with_reader_at(30).await;

    let results = content_search(
        app,
        "/api/search/content?q=lighthouse&spoiler_filter=exclude",
        &token,
    )
    .await;

    assert!(results.hits.is_empty(), "the payoff is ahead of the reader");
    assert_eq!(
        results.withheld_ahead,
        Some(1),
        "the count is what lets a caller say an answer exists without showing it"
    );
}

#[tokio::test]
async fn api_get_search_content_exclude_keeps_a_hit_the_reader_has_already_passed() {
    let (app, _pool, token) = fixture_with_reader_at(60).await;

    let results = content_search(
        app,
        "/api/search/content?q=lighthouse&spoiler_filter=exclude",
        &token,
    )
    .await;

    assert_eq!(results.hits.len(), 1, "setup the reader has read is safe");
    assert_eq!(results.withheld_ahead, Some(0));
}

/// The uuid of the one seeded book.
async fn sole_uuid(pool: &sqlx::SqlitePool) -> String {
    sqlx::query_scalar("SELECT uuid FROM books LIMIT 1")
        .fetch_one(pool)
        .await
        .unwrap()
}
