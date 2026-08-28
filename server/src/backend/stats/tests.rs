//! Tests for the reading-stats REST handler. The `db::stats` cache is
//! process-wide and keyed on `(user_id, range)`, and every fixture pool
//! restarts user ids at 1 — so each content-asserting test uses a distinct
//! range to keep its cache key unique across the test binary.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{ReadingGoal, SessionFormat, SessionLogPage, StatsRange, StatsSummary};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

async fn seed_reading_session(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

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

// --- GET /api/library-size ----------------------------------------------

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

// --- PUT /api/stats/goal ------------------------------------------------

fn put_goal_req(token: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri("/api/stats/goal")
        .method("PUT")
        .header("content-type", "application/json")
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body.to_string()))
        .unwrap()
}

async fn goal_body(res: axum::response::Response) -> Option<ReadingGoal> {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_put_stats_goal_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats/goal")
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::json!({ "target": 24 }).to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// The happy path plus AC5: the summary read straight after the write already
/// carries the new target rather than the pre-save cached one. Uses
/// `range=week` — a range no other test in this binary caches.
#[tokio::test]
async fn api_put_stats_goal_saves_and_the_next_summary_read_reflects_it() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user.id)
    .bind(&uuid)
    .bind(now_secs())
    .bind(now_secs())
    .execute(&pool)
    .await
    .unwrap();

    // Warm the cache with the goal-less answer first, so a stale read would
    // be visible below.
    let warm = app
        .clone()
        .oneshot(get_with_bearer("/api/stats?range=week", &token))
        .await
        .unwrap();
    assert_eq!(warm.status(), StatusCode::OK);

    let res = app
        .clone()
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 24 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let goal = goal_body(res).await.expect("a set target returns a goal");
    assert_eq!(goal.target, 24);
    assert_eq!(goal.current, 1);

    let res = app
        .oneshot(get_with_bearer("/api/stats?range=week", &token))
        .await
        .unwrap();
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let summary: StatsSummary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(summary.goal.map(|g| g.target), Some(24));
}

#[tokio::test]
async fn api_put_stats_goal_with_a_null_target_clears_the_goal() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    app.clone()
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 12 })))
        .await
        .unwrap();
    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": null })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(goal_body(res).await.is_none());
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_out_of_range_target() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 0 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_unsupported_kind() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(
            &token,
            serde_json::json!({ "kind": "pages", "target": 500 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_rejects_an_out_of_range_year() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(put_goal_req(
            &token,
            serde_json::json!({ "year": 1234, "target": 12 }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_put_stats_goal_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE reading_goals")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let res = app
        .oneshot(put_goal_req(&token, serde_json::json!({ "target": 24 })))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

// --- GET /api/library-composition ---------------------------------------

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

// --- GET /api/stats/sessions --------------------------------------------

/// The session log is uncached (unlike the aggregate above), so these tests
/// share `now_secs()` freely without colliding on a cache key.
async fn seed_listening_session(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn api_get_session_log_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(
            Request::builder()
                .uri("/api/stats/sessions")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_session_log_rejects_a_malformed_before_cursor() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            "/api/stats/sessions?before=nonsense",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_session_log_returns_an_empty_page_for_a_new_user() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn api_get_session_log_returns_stitched_sittings_newest_first() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 100_000;
    // One continuous sit flushed as ten heartbeat rows, then a later listen.
    for i in 0..10 {
        seed_reading_session(&pool, user.id, &uuid, t0 + i * 60, 60).await;
    }
    seed_listening_session(&pool, user.id, &uuid, t0 + 50_000, 1_800).await;

    let res = app
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].format, SessionFormat::Listening);
    assert_eq!(page.entries[0].seconds, 1_800);
    assert_eq!(page.entries[1].format, SessionFormat::Reading);
    assert_eq!(page.entries[1].seconds, 600);
    assert_eq!(page.entries[1].title, "Book A");
}

#[tokio::test]
async fn api_get_session_log_pages_through_the_before_cursor() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 500_000;
    for i in 0..3 {
        seed_reading_session(&pool, user.id, &uuid, t0 + i * 100_000, 600 + i).await;
    }

    let res = app
        .clone()
        .oneshot(get_with_bearer("/api/stats/sessions?limit=2", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let first: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(first.entries.len(), 2);
    let cursor = first.next_before.clone().unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/stats/sessions?limit=2&before={cursor}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let second: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].seconds, 600);
    assert_eq!(second.next_before, None);
}

#[tokio::test]
async fn api_get_session_log_scopes_to_the_requested_book() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid_a) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let (_, uuid_b) = seed_book_with_uuid(&pool, "/lib", "Book B").await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let t0 = now_secs() - 100_000;
    seed_reading_session(&pool, user.id, &uuid_a, t0, 600).await;
    seed_reading_session(&pool, user.id, &uuid_b, t0 + 50_000, 900).await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/stats/sessions?book={uuid_a}"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].book_uuid, uuid_a);
}

/// AC7: the log is keyed on the token's user, never on a parameter — so one
/// reader's token can't surface another's sittings on any of the surfaces.
#[tokio::test]
async fn api_get_session_log_never_serves_another_users_sessions() {
    let (app, _state, pool) = fixture().await;
    let (_, uuid) = seed_book_with_uuid(&pool, "/lib", "Book A").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;
    seed_reading_session(&pool, alice.id, &uuid, now_secs() - 10_000, 600).await;

    for uri in [
        "/api/stats/sessions".to_string(),
        format!("/api/stats/sessions?book={uuid}"),
    ] {
        let res = app
            .clone()
            .oneshot(get_with_bearer(&uri, &bob_token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let page: SessionLogPage = serde_json::from_slice(&bytes).unwrap();
        assert!(page.entries.is_empty(), "{uri} leaked alice's sessions");
    }
}

#[tokio::test]
async fn api_get_session_log_returns_500_when_db_fails() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

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
        .oneshot(get_with_bearer("/api/stats/sessions", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
