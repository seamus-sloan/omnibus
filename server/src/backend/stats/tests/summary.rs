//! `GET /api/stats` and the library figures: auth, the range parameter
//! (unknown, default, honored), the declared offset cutting the day, the
//! fallback for an unusable offset, library size and composition with
//! their coverage, and the DB-failure paths.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use omnibus_shared::{StatsRange, StatsSummary};
use tower::ServiceExt;

use super::{now_secs, seed_reading_session, seed_reading_session_at_offset};
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_stats_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_stats_rejects_unknown_range() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/stats?range=fortnight", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

/// The offset is an optional hint, so every value the server can't use has to
/// mean what an absent one means — including the empty string a client sends
/// when it interpolates an offset it doesn't have. A 400 here would cost the
/// reader the whole page over a field they never had to send.
///
/// The seeded session's own offset is what each request falls back to, which
/// doubles as this test's cache key and keeps it off every other test's.
#[tokio::test]
async fn api_get_stats_falls_back_rather_than_400ing_on_an_unusable_offset() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_reading_session_at_offset(&pool, user.id, &uuid, now_secs(), 600, Some(330)).await;

    // `%20` decodes to a space — present, but saying nothing.
    for raw in ["", "abc", "12.5", "%20", "99999", "-99999"] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(
                &format!("/api/stats?range=week&utc_offset_minutes={raw}"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "utc_offset_minutes={raw:?}");
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            summary.reading_seconds, 600,
            "utc_offset_minutes={raw:?} answered, but with nothing in it"
        );
    }
}

#[tokio::test]
async fn api_get_stats_defaults_to_the_month_range() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_reading_session(&pool, user.id, &uuid, now_secs(), 600).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.range, StatsRange::Month);
    assert_eq!(summary.reading_seconds, 600);
    assert_eq!(summary.sessions, 1);
}

#[tokio::test]
async fn api_get_stats_honors_the_range_param() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // Late-2023 anchor: inside the all-time window, outside week/month/year.
    seed_reading_session(&pool, user.id, &uuid, 1_700_000_000, 900).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=all_time", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.range, StatsRange::AllTime);
    assert_eq!(summary.reading_seconds, 900);
}

#[tokio::test]
async fn api_get_stats_cuts_the_day_on_the_declared_offset() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // 2023-11-14 22:13:20 UTC — an evening west of UTC, already the 15th east.
    seed_reading_session(&pool, user.id, &uuid, 1_700_000_000, 900).await;

    let day_at = |offset: &str| {
        let path = format!("/api/stats?range=all_time&utc_offset_minutes={offset}");
        let app = app.clone();
        let token = token.clone();
        async move {
            let res = app.oneshot(get_with_bearer(&path, &token)).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK);
            let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
            let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
            summary.heatmap[0].day.clone()
        }
    };

    // The same session, filed on two different days — the whole point of the
    // caller declaring where it is. Distinct offsets also prove the aggregate
    // cache is keyed on it, since a shared key would serve the first answer
    // to the second call.
    assert_eq!(day_at("-480").await, "2023-11-14");
    assert_eq!(day_at("540").await, "2023-11-15");
}

/// Mirrors the ebooks/audiobooks sibling suites' DROP TABLE pattern: a DB
/// failure inside `db::stats` must surface as a 500. Uses `range=year` —
/// a range no other test caches — so the process-wide stats cache can't
/// serve a stale 200 here.
#[tokio::test]
async fn api_get_stats_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // FKs off before the DROP so references to the sessions table can't
    // turn the drop itself into an error (same as the ebooks suite).
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=year", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_get_library_size_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/library-size")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// The success and DB-failure paths in **one test**, unlike their `/api/stats`
/// siblings. Those keep themselves apart by picking a distinct `StatsRange`,
/// because that cache is keyed on `(user_id, range)`. The library-size cache is
/// a single process-wide entry with nothing to key on, so two tests clearing
/// and repopulating it in the same process race — and the DB-failure one would
/// be served the other's cached 200. Sequencing them inside one test is what
/// removes the interleaving rather than relying on the runner's isolation.
#[tokio::test]
async fn api_get_library_size_reports_coverage_then_500s_when_the_db_fails() {
    let (app, state, pool) = fixture().await;
    let (book_id, _) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    sqlx::query("UPDATE books SET word_count = 275 WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::stats::invalidate_library_size();

    let res = crate::backend::rest_router(state.clone())
        .oneshot(get_with_bearer("/api/library-size", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let size: omnibus_shared::LibrarySize = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(size.books, 1);
    assert_eq!(size.words.total, 275);
    assert_eq!(size.words.books, 1, "the total must carry its denominator");

    // FKs off before the DROP so references to `books` can't turn the drop
    // itself into an error (same as the ebooks suite).
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);
    // Without this the handler would serve the 200 cached moments ago and the
    // failure path would never run.
    omnibus_db::stats::invalidate_library_size();

    let res = app
        .oneshot(get_with_bearer("/api/library-size", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    // Leave nothing cached from a dropped-table pool for whatever runs next.
    omnibus_db::stats::invalidate_library_size();
}

#[tokio::test]
async fn api_get_library_composition_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/library-composition")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_library_composition_returns_dimensions_with_their_coverage() {
    let (app, _state, pool) = fixture().await;
    seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // Cached library-wide across the whole test binary, so this test clears
    // it rather than picking a unique key the way the per-user ones do.
    omnibus_db::stats::invalidate_library_composition();

    let res = app
        .oneshot(get_with_bearer("/api/library-composition", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let composition: omnibus_shared::LibraryComposition = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(composition.books, 1);
    assert_eq!(composition.ghosted_books, 0);
    assert_eq!(composition.formats.coverage.books, 1);
    // No publisher metadata anywhere: an empty dimension, not an empty chart.
    assert!(composition.publishers.is_empty());
}

#[tokio::test]
async fn api_get_library_composition_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    omnibus_db::stats::invalidate_library_composition();

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(get_with_bearer("/api/library-composition", &token))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
    omnibus_db::stats::invalidate_library_composition();
}
