//! Unit tests for [`super::book_insights`]: the happy-path aggregate, the two
//! `None` (em-dash) paths — no sessions, unresolvable uuid — user isolation,
//! the merged-uuid resolve, and the propagated `StatsError::Sqlx` variant.

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn reading_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
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

async fn listening_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions (user_id, book_uuid, started_at, ended_at, seconds_listened)
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

const T0: i64 = 1_700_000_000;

#[tokio::test]
async fn book_insights_returns_none_when_book_has_no_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    assert_eq!(book_insights(&pool, user, "uuid-1").await.unwrap(), None);
}

#[tokio::test]
async fn book_insights_returns_none_when_uuid_does_not_resolve_to_a_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert_eq!(
        book_insights(&pool, user, "no-such-uuid").await.unwrap(),
        None
    );
}

#[tokio::test]
async fn book_insights_aggregates_started_time_and_session_count_across_formats() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    listening_session(&pool, user, "uuid-1", T0 - 3600, 300).await;

    let insights = book_insights(&pool, user, "uuid-1").await.unwrap().unwrap();

    assert_eq!(insights.started_at, T0 - 3600);
    assert_eq!(insights.seconds_total, 900);
    assert_eq!(insights.sessions, 2);
}

#[tokio::test]
async fn book_insights_only_counts_sessions_for_the_given_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    reading_session(&pool, alice, "uuid-1", T0, 600).await;
    reading_session(&pool, bob, "uuid-1", T0, 1200).await;

    let alice_insights = book_insights(&pool, alice, "uuid-1")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alice_insights.seconds_total, 600);
    assert_eq!(alice_insights.sessions, 1);

    assert_eq!(
        book_insights(&pool, bob, "uuid-1")
            .await
            .unwrap()
            .unwrap()
            .sessions,
        1
    );
}

#[tokio::test]
async fn book_insights_resolves_through_merged_uuids_to_the_canonical_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('absorbed-uuid', ?, 'EPUB', '/lib/absorbed')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    // Requested via the absorbed (merged-away) uuid; sessions were recorded
    // under the surviving book's canonical uuid.
    let insights = book_insights(&pool, user, "absorbed-uuid")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(insights.sessions, 1);
    assert_eq!(insights.seconds_total, 600);
}

#[tokio::test]
async fn book_insights_propagates_sqlx_error_when_sessions_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = book_insights(&pool, user, "uuid-1").await.unwrap_err();
    assert!(matches!(err, StatsError::Sqlx(_)), "got: {err:?}");
}
