//! Unit tests for the read-time offset resolver: claim-beats-fallback,
//! fallback ordering, and the two ways an implausible offset is discarded.

use crate::init_db;
use crate::test_support::seed_user;

use super::*;

/// 2023-11-14 22:13:20 UTC.
const T0: i64 = 1_700_000_000;

/// Insert a session row directly.
///
/// Deliberately not through `progress::record_session`: that resolves a
/// canonical `books.uuid` and would drag a whole seeded library into tests about
/// one integer column. The offset column is what is under test, not the path
/// that fills it.
async fn seed_session(pool: &SqlitePool, user_id: i64, started_at: i64, offset: Option<i64>) {
    sqlx::query(
        "INSERT INTO reading_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_read, utc_offset_minutes)
         VALUES (?, 'uuid-a', ?, ?, 600, ?)",
    )
    .bind(user_id)
    .bind(started_at)
    .bind(started_at + 600)
    .bind(offset)
    .execute(pool)
    .await
    .unwrap();
}

async fn seed_listening(pool: &SqlitePool, user_id: i64, started_at: i64, offset: Option<i64>) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened, utc_offset_minutes)
         VALUES (?, 'uuid-a', ?, ?, 600, ?)",
    )
    .bind(user_id)
    .bind(started_at)
    .bind(started_at + 600)
    .bind(offset)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn resolve_offset_minutes_prefers_the_claim_over_the_stored_session() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, Some(540)).await;

    // The claim says where the reader is now; the session only says where they
    // were when they last read.
    let got = resolve_offset_minutes(&pool, user, Some(-420))
        .await
        .unwrap();

    assert_eq!(got, -420);
}

#[tokio::test]
async fn resolve_offset_minutes_falls_back_to_the_latest_session_when_unclaimed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, Some(540)).await;
    seed_session(&pool, user, T0 + 86_400, Some(-420)).await;

    let got = resolve_offset_minutes(&pool, user, None).await.unwrap();

    assert_eq!(got, -420, "most recent wins, not first-seen");
}

#[tokio::test]
async fn resolve_offset_minutes_compares_across_both_session_tables() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, Some(540)).await;
    seed_listening(&pool, user, T0 + 86_400, Some(-420)).await;

    // Each table is probed separately and the newer of the two wins; a probe
    // that only looked at reading sessions would report the stale 540 here.
    let got = resolve_offset_minutes(&pool, user, None).await.unwrap();

    assert_eq!(got, -420);
}

#[tokio::test]
async fn resolve_offset_minutes_falls_back_to_utc_when_no_session_carries_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, None).await;

    let got = resolve_offset_minutes(&pool, user, None).await.unwrap();

    assert_eq!(got, 0);
}

#[tokio::test]
async fn resolve_offset_minutes_discards_an_out_of_range_claim() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, Some(540)).await;

    // Past UTC+14:00. An absurd offset does not produce an obviously wrong
    // value — it silently relabels which day the reader's sessions fall on — so
    // it is dropped in favour of the fallback rather than trusted.
    let got = resolve_offset_minutes(&pool, user, Some(20_000))
        .await
        .unwrap();

    assert_eq!(got, 540);
}

#[tokio::test]
async fn current_offset_minutes_is_none_for_a_user_with_no_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;

    assert_eq!(current_offset_minutes(&pool, user).await.unwrap(), None);
}

#[tokio::test]
async fn current_offset_minutes_ignores_an_implausible_stored_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    // `SessionReport::validate` would reject this today, but rows predating that
    // check exist and one of them must not relabel a reader's whole calendar.
    seed_session(&pool, user, T0, Some(99_999)).await;

    assert_eq!(current_offset_minutes(&pool, user).await.unwrap(), None);
}

#[tokio::test]
async fn current_offset_minutes_falls_back_past_an_implausible_newer_session() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    seed_session(&pool, user, T0, Some(540)).await;
    seed_session(&pool, user, T0 + 86_400, Some(99_999)).await;

    // The bad row is the newest, so range-checking the *winner* would drop the
    // reader to UTC rather than to where they last plausibly read. The probe
    // skips it and the previous good session answers instead.
    assert_eq!(
        current_offset_minutes(&pool, user).await.unwrap(),
        Some(540)
    );
}

#[tokio::test]
async fn current_offset_minutes_is_scoped_to_one_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mine = seed_user(&pool, "mine").await;
    let theirs = seed_user(&pool, "theirs").await;
    seed_session(&pool, theirs, T0, Some(540)).await;

    assert_eq!(current_offset_minutes(&pool, mine).await.unwrap(), None);
}

#[tokio::test]
async fn today_moves_with_the_offset_across_the_day_boundary() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let east = today(&pool, 14 * 60).await.unwrap();
    let west = today(&pool, -12 * 60).await.unwrap();

    // The extremes are 26 hours apart, so they can never name the same day —
    // which is exactly why "today" has to be answered per request rather than
    // once for the server.
    assert!(east > west, "east must lead west: {east} vs {west}");
}
