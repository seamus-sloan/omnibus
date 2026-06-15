//! Unit tests for the `progress` module: `upsert_progress` roundtrip,
//! last-write-wins, per-user/format isolation, `BookNotFound`,
//! `get_progress` empty-state, `record_session` per-format dispatch and
//! unknown-uuid skip, merged-uuid resolution, and `record_session_tx`
//! rollback.

use super::*;
use crate::{init_db, replace_books};
use omnibus_shared::EbookMetadata;

/// Map a merged/auto-attached `uuid` onto an existing `book_id` the way the
/// merge transaction does (`db/src/merge/transaction.rs`), so the session path
/// has a row to resolve through the `merged_uuids` UNION fallback.
async fn seed_merged_uuid(pool: &SqlitePool, uuid: &str, book_id: i64, format: &str) {
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, ?, ?)",
    )
    .bind(uuid)
    .bind(book_id)
    .bind(format)
    .bind("/lib")
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> (i64, String) {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap();
    (book.id, book.unique_identifier.clone().unwrap())
}

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

#[tokio::test]
async fn upsert_round_trips_epub_position() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let upd = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
    };
    let saved = upsert_progress(&pool, user, &upd).await.unwrap();
    assert_eq!(saved.book_uuid, uuid);
    assert_eq!(saved.format, ProgressFormat::Epub);
    assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(/6/4!/4/2/1:0)"));
    assert!(saved.updated_at > 0);

    let fetched = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.epub_cfi, saved.epub_cfi);
}

#[tokio::test]
async fn upsert_is_last_write_wins() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let first = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
    };
    upsert_progress(&pool, user, &first).await.unwrap();
    let second = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/12!/4/8/3:7)".into()),
        audio_position_seconds: None,
    };
    let saved = upsert_progress(&pool, user, &second).await.unwrap();
    assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
}

#[tokio::test]
async fn isolates_per_user_book_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        alice,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(alice)".into()),
            audio_position_seconds: None,
        },
    )
    .await
    .unwrap();
    upsert_progress(
        &pool,
        alice,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(42.5),
        },
    )
    .await
    .unwrap();
    // Bob has no row yet.
    assert!(get_progress(&pool, bob, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .is_none());
    // Alice's two rows don't trample each other.
    let epub = get_progress(&pool, alice, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    let audio = get_progress(&pool, alice, &uuid, ProgressFormat::Audio)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(epub.epub_cfi.as_deref(), Some("epubcfi(alice)"));
    assert_eq!(audio.audio_position_seconds, Some(42.5));
}

#[tokio::test]
async fn upsert_unknown_book_is_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let res = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: "no-such-uuid".into(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(x)".into()),
            audio_position_seconds: None,
        },
    )
    .await;
    assert!(matches!(res, Err(ProgressError::BookNotFound)));
}

#[tokio::test]
async fn get_progress_returns_none_when_unset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    assert!(get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .is_none());
}

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
async fn progress_survives_hard_delete_of_book() {
    // F1: the soft-ref (`book_uuid TEXT`, no FK, no cascade) means deleting
    // the `books` row leaves the user's reading position intact — the
    // durability guarantee the old `book_id … ON DELETE CASCADE` violated.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
        },
    )
    .await
    .unwrap();

    // Hard-delete the books row — what a cascade-deleting reindex (or a future
    // GC) would do. Pre-F1 this cascaded the progress away.
    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    let surviving: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM reading_progress WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        surviving, 1,
        "reading_progress must survive a hard delete of its book (no cascade)"
    );
}
