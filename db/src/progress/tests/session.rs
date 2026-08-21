//! `record_session` / `record_session_tx` / `insert_session_tx`: per-format
//! row inserts, merged-uuid resolution, transaction rollback, and the
//! client-id replay idempotency the partial unique index provides.

use omnibus_shared::SessionReport;

use crate::init_db;

use super::super::*;
use super::{seed, seed_merged_uuid, seed_user};

#[tokio::test]
async fn record_session_inserts_per_format_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    record_session(
        &pool,
        user,
        &SessionReport {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            started_at: 100,
            ended_at: 460,
            progress_units: 360,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);

    record_session(
        &pool,
        user,
        &SessionReport {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            started_at: 200,
            ended_at: 800,
            progress_units: 600,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    let audio_count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listening_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(audio_count, 1);

    // Unknown uuid → skipped, returns false so the REST handler can
    // count only the rows that actually landed (issue: copilot review
    // on #300).
    let skipped = record_session(
        &pool,
        user,
        &SessionReport {
            book_uuid: "no-such-uuid".into(),
            format: ProgressFormat::Epub,
            started_at: 0,
            ended_at: 10,
            progress_units: 10,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    assert!(!skipped, "unknown uuid should be skipped (returns false)");
}

#[tokio::test]
async fn record_session_tx_inserts_row_when_committed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;

    let mut tx = pool.begin().await.unwrap();
    let inserted = record_session_tx(
        &mut tx,
        user,
        &SessionReport {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            started_at: 100,
            ended_at: 460,
            progress_units: 360,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    assert!(inserted);
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn insert_session_tx_inserts_epub_row_against_pre_resolved_uuid() {
    // Batch writers (`post_sessions`, issue #633) pre-resolve every uuid via
    // `resolve_canonical_book_uuids_bulk_exec` and hand the canonical string
    // to `insert_session_tx`. This test asserts that path — no per-row
    // SELECT — still lands a row against the survivor uuid.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;

    let mut tx = pool.begin().await.unwrap();
    insert_session_tx(
        &mut tx,
        user,
        &SessionReport {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            started_at: 100,
            ended_at: 460,
            progress_units: 360,
            device_id: None,
            client_id: None,
        },
        &uuid,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn insert_session_tx_inserts_audio_row_against_pre_resolved_uuid() {
    // Per-format dispatch counterpart: audio reports must route into
    // `listening_sessions` when the caller pre-resolves the uuid.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;

    let mut tx = pool.begin().await.unwrap();
    insert_session_tx(
        &mut tx,
        user,
        &SessionReport {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            started_at: 200,
            ended_at: 800,
            progress_units: 600,
            device_id: None,
            client_id: None,
        },
        &uuid,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listening_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn record_session_tx_rollback_leaves_no_rows() {
    // When the transaction is dropped without committing, no rows must
    // remain — this is the invariant post_sessions relies on when a
    // mid-batch error forces an early return.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;

    {
        let mut tx = pool.begin().await.unwrap();
        record_session_tx(
            &mut tx,
            user,
            &SessionReport {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Epub,
                started_at: 100,
                ended_at: 460,
                progress_units: 360,
                device_id: None,
                client_id: None,
            },
        )
        .await
        .unwrap();
        // tx is dropped here without commit → implicit rollback
    }

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 0, "dropped transaction must leave no committed rows");
}

#[tokio::test]
async fn migration_0020_adds_windowed_session_indexes_for_stats() {
    // F3.4 stats range-scans sessions by `(user_id, started_at)` and the
    // progress rail orders by `(user_id, updated_at)`. Assert migration
    // 0020 created each index so those windowed queries can use them.
    let pool = init_db("sqlite::memory:").await.unwrap();
    for index in [
        "idx_reading_sessions_user_started",
        "idx_listening_sessions_user_started",
        "idx_reading_progress_user_updated",
    ] {
        let found: Option<String> =
            sqlx::query_scalar("SELECT name FROM sqlite_master WHERE type = 'index' AND name = ?")
                .bind(index)
                .fetch_optional(&pool)
                .await
                .unwrap();
        assert_eq!(found.as_deref(), Some(index), "missing index {index}");
    }
}

#[tokio::test]
async fn record_session_resolves_merged_uuid_and_records_against_canonical_book() {
    // A uuid that only exists in `merged_uuids` (the file was format-merged
    // into the surviving book after the session started) must resolve to the
    // canonical book and record the session, not be dropped with Ok(false).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, survivor_uuid) = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "merged-uuid", book_id, "epub").await;

    let recorded = record_session(
        &pool,
        user,
        &SessionReport {
            book_uuid: "merged-uuid".into(),
            format: ProgressFormat::Epub,
            started_at: 100,
            ended_at: 460,
            progress_units: 360,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    assert!(recorded, "merged uuid should resolve and record, not skip");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&survivor_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "session must land against the canonical book");
}

#[tokio::test]
async fn record_session_resolves_merged_audio_uuid_to_canonical_book() {
    // Per-format dispatch counterpart: a merged audio uuid records into
    // `listening_sessions` against the surviving book.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, survivor_uuid) = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "merged-audio-uuid", book_id, "audio").await;

    let recorded = record_session(
        &pool,
        user,
        &SessionReport {
            book_uuid: "merged-audio-uuid".into(),
            format: ProgressFormat::Audio,
            started_at: 200,
            ended_at: 800,
            progress_units: 600,
            device_id: None,
            client_id: None,
        },
    )
    .await
    .unwrap();
    assert!(recorded, "merged audio uuid should resolve and record");

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM listening_sessions WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&survivor_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        count, 1,
        "listening session must land against the canonical book"
    );
}

#[tokio::test]
async fn record_session_replay_with_same_client_id_inserts_one_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let report = SessionReport {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 460,
        progress_units: 360,
        device_id: None,
        client_id: Some("session-abc".into()),
    };

    // The reply to the first post is lost, so the client replays the very
    // same report. It must land once, not twice.
    record_session(&pool, user, &report).await.unwrap();
    record_session(&pool, user, &report).await.unwrap();

    let (count, total): (i64, i64) = sqlx::query_as(
        "SELECT COUNT(*), COALESCE(SUM(seconds_read), 0) FROM reading_sessions
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1, "a replayed report must not add a second row");
    assert_eq!(total, 360, "reading time must not be double-counted");
}

#[tokio::test]
async fn record_session_scopes_client_id_per_user_and_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    let report = |format| SessionReport {
        book_uuid: uuid.clone(),
        format,
        started_at: 100,
        ended_at: 460,
        progress_units: 360,
        device_id: None,
        client_id: Some("shared-handle".into()),
    };

    record_session(&pool, alice, &report(ProgressFormat::Epub))
        .await
        .unwrap();
    record_session(&pool, bob, &report(ProgressFormat::Epub))
        .await
        .unwrap();
    // Same handle, other table: the two indexes are independent, so an
    // audio session is never suppressed by a reading one.
    record_session(&pool, alice, &report(ProgressFormat::Audio))
        .await
        .unwrap();

    let reading: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    let listening: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listening_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        reading, 2,
        "one uuid per user must not collide across users"
    );
    assert_eq!(listening, 1);
}

#[tokio::test]
async fn record_session_without_client_id_still_inserts_every_report() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    // Web posts once on unmount and never retries, so it sends no handle —
    // the partial index must leave those rows unconstrained rather than
    // collapsing two genuine sessions into one.
    let report = SessionReport {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        started_at: 100,
        ended_at: 460,
        progress_units: 360,
        device_id: None,
        client_id: None,
    };
    record_session(&pool, user, &report).await.unwrap();
    record_session(&pool, user, &report).await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_sessions")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}
