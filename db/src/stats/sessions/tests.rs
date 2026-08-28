//! Unit tests for [`super::session_log`]: the stitch (a continuous sit is one
//! entry, an idle gap splits, both tables fold into one mixed sitting), the
//! keyset cursor, book scoping through a merged uuid, user isolation, the
//! empty page, and the propagated `StatsError::Sqlx`.

use omnibus_shared::SessionFormat;

use super::*;
use crate::init_db;
use crate::test_support::seed_minimal_books;

const T0: i64 = 1_700_000_000;

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

/// A whole page of one user's log, no book scope and no cursor.
async fn log(pool: &SqlitePool, user: i64) -> SessionLogPage {
    session_log(pool, user, None, None, SESSION_LOG_DEFAULT_LIMIT)
        .await
        .unwrap()
}

#[tokio::test]
async fn session_log_returns_an_empty_page_for_a_user_with_no_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

    let page = log(&pool, user).await;
    assert!(page.entries.is_empty());
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn session_log_renders_a_continuous_sit_as_one_entry() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Two hours of reading flushed as 120 heartbeat rows, one per minute.
    for i in 0..120 {
        reading_session(&pool, user, "uuid-1", T0 + i * 60, 60).await;
    }

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 1);
    let entry = &page.entries[0];
    assert_eq!(entry.seconds, 7_200);
    assert_eq!(entry.started_at, T0);
    assert_eq!(entry.ended_at, T0 + 119 * 60 + 60);
    assert_eq!(entry.format, SessionFormat::Reading);
    assert_eq!(entry.title, "Title 1");
}

#[tokio::test]
async fn session_log_splits_sittings_across_an_idle_gap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    // Well past the idle threshold — a second sitting, not a continuation.
    reading_session(
        &pool,
        user,
        "uuid-1",
        T0 + 600 + 2 * sessionize::IDLE_GAP_SECS,
        900,
    )
    .await;

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 2);
    // Newest first.
    assert_eq!(page.entries[0].seconds, 900);
    assert_eq!(page.entries[1].seconds, 600);
}

#[tokio::test]
async fn session_log_reports_a_sitting_fed_by_both_tables_as_mixed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    listening_session(&pool, user, "uuid-1", T0 + 600, 600).await;

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].format, SessionFormat::Mixed);
    assert_eq!(page.entries[0].seconds, 1_200);
}

#[tokio::test]
async fn session_log_reports_a_listening_only_sitting_as_listening() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    listening_session(&pool, user, "uuid-1", T0, 1_800).await;

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].format, SessionFormat::Listening);
}

#[tokio::test]
async fn session_log_orders_newest_first_across_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-2", T0 + 10_000, 600).await;
    reading_session(&pool, user, "uuid-3", T0 + 20_000, 600).await;

    let uuids: Vec<String> = log(&pool, user)
        .await
        .entries
        .into_iter()
        .map(|e| e.book_uuid)
        .collect();
    assert_eq!(uuids, ["uuid-3", "uuid-2", "uuid-1"]);
}

#[tokio::test]
async fn session_log_drops_sittings_under_the_glance_floor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    // A glance — opened and closed again, below the sitting floor.
    reading_session(&pool, user, "uuid-2", T0 + 10_000, 5).await;

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].book_uuid, "uuid-1");
}

#[tokio::test]
async fn session_log_pages_forward_through_the_cursor_without_repeats_or_gaps() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    // Five separated sittings, 100s apart in recorded length so each is
    // identifiable in the paged sequence.
    for i in 0..5 {
        reading_session(
            &pool,
            user,
            "uuid-1",
            T0 + i * 10 * sessionize::IDLE_GAP_SECS,
            100 + i,
        )
        .await;
    }

    let mut seen = Vec::new();
    let mut cursor = None;
    loop {
        let page = session_log(&pool, user, None, cursor.as_ref(), 2)
            .await
            .unwrap();
        assert!(page.entries.len() <= 2);
        seen.extend(page.entries.iter().map(|e| e.seconds));
        match page.next_before {
            Some(raw) => cursor = Some(SessionCursor::parse(&raw).unwrap()),
            None => break,
        }
    }
    assert_eq!(seen, [104, 103, 102, 101, 100]);
}

#[tokio::test]
async fn session_log_withholds_a_cursor_on_an_exactly_full_final_page() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    for i in 0..2 {
        reading_session(
            &pool,
            user,
            "uuid-1",
            T0 + i * 10 * sessionize::IDLE_GAP_SECS,
            600,
        )
        .await;
    }

    let page = session_log(&pool, user, None, None, 2).await.unwrap();
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn session_log_cursor_advances_past_a_tie_between_two_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    // Same second, two books — a started_at-only cursor would either drop
    // one of these or serve it forever.
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-2", T0, 600).await;

    let first = session_log(&pool, user, None, None, 1).await.unwrap();
    assert_eq!(first.entries.len(), 1);
    assert_eq!(first.entries[0].book_uuid, "uuid-2");
    let cursor = SessionCursor::parse(&first.next_before.unwrap()).unwrap();

    let second = session_log(&pool, user, None, Some(&cursor), 1)
        .await
        .unwrap();
    assert_eq!(second.entries.len(), 1);
    assert_eq!(second.entries[0].book_uuid, "uuid-1");
    assert_eq!(second.next_before, None);
}

#[tokio::test]
async fn session_log_clamps_a_limit_above_the_server_ceiling() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    for i in 0..(SESSION_LOG_MAX_LIMIT + 5) {
        reading_session(
            &pool,
            user,
            "uuid-1",
            T0 + i * 10 * sessionize::IDLE_GAP_SECS,
            600,
        )
        .await;
    }

    let page = session_log(&pool, user, None, None, 10_000).await.unwrap();
    assert_eq!(page.entries.len() as i64, SESSION_LOG_MAX_LIMIT);
    assert!(page.next_before.is_some());
}

#[tokio::test]
async fn session_log_scopes_to_one_book_when_asked() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    reading_session(&pool, user, "uuid-2", T0 + 10_000, 900).await;

    let page = session_log(&pool, user, Some("uuid-1"), None, SESSION_LOG_DEFAULT_LIMIT)
        .await
        .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].book_uuid, "uuid-1");
    assert_eq!(page.entries[0].seconds, 600);
}

#[tokio::test]
async fn session_log_resolves_a_merged_away_uuid_to_the_surviving_book() {
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

    // Asked for via the absorbed uuid; the sitting lives on the survivor.
    let page = session_log(
        &pool,
        user,
        Some("absorbed-uuid"),
        None,
        SESSION_LOG_DEFAULT_LIMIT,
    )
    .await
    .unwrap();
    assert_eq!(page.entries.len(), 1);
    assert_eq!(page.entries[0].book_uuid, "uuid-1");
}

/// The log reads `book_uuid` straight off the row, so the sittings it counts
/// for a book are exactly the ones `book_insights` counts. Nothing can write a
/// non-canonical uuid — `record_session_tx` resolves before it inserts and the
/// merge retargets both session tables — so this is unreachable state; the
/// point of pinning it is that a stray row must not be *folded into* a real
/// book, which would make the log claim a sitting Pickups does not.
#[tokio::test]
async fn session_log_keeps_a_row_under_a_merged_away_uuid_out_of_the_survivor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;

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
    // Contiguous in time, so a per-row resolve would stitch them into one.
    reading_session(&pool, user, "absorbed-uuid", T0, 600).await;
    reading_session(&pool, user, "uuid-1", T0 + 600, 600).await;

    // User-wide: two separate sittings, which is what `compute::session_count`
    // counts over the same union — the two figures cannot disagree.
    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].book_uuid, "uuid-1");
    assert_eq!(page.entries[0].seconds, 600);
    assert_eq!(page.entries[1].book_uuid, "absorbed-uuid");
    assert_eq!(page.entries[1].title, "Untitled");

    // Book-scoped: only the canonical row, matching what the Pickups figure
    // beside it reports for this book.
    let scoped = session_log(&pool, user, Some("uuid-1"), None, 25)
        .await
        .unwrap();
    assert_eq!(scoped.entries.len(), 1);
    assert_eq!(scoped.entries[0].seconds, 600);
}

#[tokio::test]
async fn session_log_returns_an_empty_page_for_an_unresolvable_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;

    let page = session_log(
        &pool,
        user,
        Some("no-such-uuid"),
        None,
        SESSION_LOG_DEFAULT_LIMIT,
    )
    .await
    .unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.next_before, None);
}

#[tokio::test]
async fn session_log_never_shows_another_users_sessions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    reading_session(&pool, alice, "uuid-1", T0, 600).await;

    assert_eq!(log(&pool, alice).await.entries.len(), 1);
    assert!(log(&pool, bob).await.entries.is_empty());
    // Scoped to the same book, Bob still sees nothing.
    let scoped = session_log(&pool, bob, Some("uuid-1"), None, SESSION_LOG_DEFAULT_LIMIT)
        .await
        .unwrap();
    assert!(scoped.entries.is_empty());
}

#[tokio::test]
async fn session_log_names_the_book_and_falls_back_when_it_is_gone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    reading_session(&pool, user, "uuid-1", T0, 600).await;
    // A sitting on a book the library no longer carries still has to render.
    reading_session(&pool, user, "ghost-uuid", T0 + 100_000, 600).await;

    let page = log(&pool, user).await;
    assert_eq!(page.entries.len(), 2);
    assert_eq!(page.entries[0].title, "Untitled");
    assert_eq!(page.entries[1].title, "Title 1");
}

#[tokio::test]
async fn session_log_propagates_sqlx_error_when_sessions_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "alice").await;
    sqlx::query("DROP TABLE reading_sessions")
        .execute(&pool)
        .await
        .unwrap();

    let err = session_log(&pool, user, None, None, SESSION_LOG_DEFAULT_LIMIT)
        .await
        .unwrap_err();
    assert!(matches!(err, StatsError::Sqlx(_)), "unexpected: {err:?}");
}
