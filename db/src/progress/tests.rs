//! Unit tests for the `progress` module: `upsert_progress` most-recent-wins
//! by client event time (rejection/acceptance/clamping/NULL handling),
//! per-user/format isolation, `get_progress`/`recent_progress` ordering and
//! empty-state, `record_session` dispatch and merged-uuid resolution, and
//! `record_session_tx` rollback, and client-id replay idempotency.

use omnibus_shared::EbookMetadata;

use super::*;
use crate::{init_db, replace_books};

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
            word_count: None,
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
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
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
async fn upsert_round_trips_a_percent_only_epub_position() {
    // The Kobo shape: a percent and an opaque location, no CFI. Before #925
    // the row CHECK made this row impossible to store at all.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let loc = r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"kobo.9.1"}"#;
    let upd = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: Some(37),
        kobo_location: Some(loc.into()),
        client_updated_at: None,
        book_file_id: None,
    };

    let saved = upsert_progress(&pool, user, &upd).await.unwrap();

    assert_eq!(saved.progress_percent, Some(37));
    assert_eq!(saved.kobo_location.as_deref(), Some(loc));
    assert_eq!(saved.epub_cfi, None);
    let fetched = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.progress_percent, Some(37));
    assert_eq!(fetched.kobo_location.as_deref(), Some(loc));
}

#[tokio::test]
async fn upsert_replaces_a_web_cfi_row_with_a_kobo_position_atomically() {
    // An accepted write replaces the whole position: a newer Kobo bookmark
    // must not leave the older web CFI dangling next to it (the row would
    // then describe two different places at once).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let replaced = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(55),
            kobo_location: Some("{}".into()),
            client_updated_at: Some(200),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(replaced.epub_cfi, None, "stale web CFI must be cleared");
    assert_eq!(replaced.progress_percent, Some(55));
    assert_eq!(replaced.kobo_location.as_deref(), Some("{}"));
}

#[tokio::test]
async fn upsert_replaces_a_kobo_position_row_with_a_web_cfi_atomically() {
    // The mirror: a later web CFI clears the Kobo percent + span, and the
    // sync-out derivation recomputes them from the CFI on demand.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(20),
            kobo_location: Some("{}".into()),
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let replaced = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/12!/4/8/3:7)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(200),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(replaced.progress_percent, None, "stale percent must clear");
    assert_eq!(replaced.kobo_location, None, "stale span must clear");
    assert_eq!(
        replaced.epub_cfi.as_deref(),
        Some("epubcfi(/6/12!/4/8/3:7)")
    );
}

#[tokio::test]
async fn upsert_rejects_a_stale_write_wholly_without_field_bleed() {
    // A rejected (older) write must not leak any of its fields into the
    // stored row — the guard is row-level, all or nothing.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(200),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let survived = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(99),
            kobo_location: Some("{}".into()),
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(survived.epub_cfi.as_deref(), Some("epubcfi(/6/4!/4/2/1:0)"));
    assert_eq!(survived.progress_percent, None);
    assert_eq!(survived.kobo_location, None);
}

#[tokio::test]
async fn attach_derived_kobo_location_fills_the_span_without_touching_clocks() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let stored = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let updated = attach_derived_kobo_location(
        &pool,
        user,
        &uuid,
        r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"kobo.2.1"}"#,
        Some(42),
        stored.client_updated_at,
    )
    .await
    .unwrap();
    assert!(updated);

    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.kobo_location.as_deref(),
        Some(r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"kobo.2.1"}"#)
    );
    assert_eq!(row.progress_percent, Some(42));
    assert_eq!(
        row.client_updated_at, stored.client_updated_at,
        "write-back must not advance the freshness clock"
    );
    assert_eq!(
        row.updated_at, stored.updated_at,
        "write-back must not advance the receipt clock"
    );
}

#[tokio::test]
async fn attach_derived_kobo_location_noops_when_the_row_moved_or_has_a_span() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let stored = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    // Wrong expected timestamp: the row moved since the caller read it.
    let updated = attach_derived_kobo_location(&pool, user, &uuid, "{}", None, 999)
        .await
        .unwrap();
    assert!(!updated);

    // Row already carries a device-authored span: never overwrite it.
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(10),
            kobo_location: Some(r#"{"Value":"kobo.9.1"}"#.into()),
            client_updated_at: Some(200),
            book_file_id: None,
        },
    )
    .await
    .unwrap();
    let updated = attach_derived_kobo_location(&pool, user, &uuid, "{}", None, 200)
        .await
        .unwrap();
    assert!(!updated);
    let row = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        row.kobo_location.as_deref(),
        Some(r#"{"Value":"kobo.9.1"}"#)
    );

    // Unknown book propagates BookNotFound like every other write path.
    let err = attach_derived_kobo_location(&pool, user, "no-such-uuid", "{}", None, 100)
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::BookNotFound));
    let _ = stored;
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
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
    };
    upsert_progress(&pool, user, &first).await.unwrap();
    let second = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/12!/4/8/3:7)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
    };
    let saved = upsert_progress(&pool, user, &second).await.unwrap();
    assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
}

/// Current unix seconds, for asserting a stored `client_updated_at` landed
/// near "now" without pinning an exact value the test doesn't control.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[tokio::test]
async fn upsert_rejects_older_client_write_and_returns_the_stored_newer_record() {
    // Issue #1362 AC1: a write whose client_updated_at is older than the
    // stored row must not win, and the caller must get back the surviving
    // (newer) record rather than an echo of its own rejected payload.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let newer = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(newer)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(2000),
    };
    upsert_progress(&pool, user, &newer).await.unwrap();

    let stale_replay = ProgressUpdate {
        book_uuid: uuid.clone(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(stale-offline-replay)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(1000),
    };
    let result = upsert_progress(&pool, user, &stale_replay).await.unwrap();

    assert_eq!(
        result.epub_cfi.as_deref(),
        Some("epubcfi(newer)"),
        "the older write must be rejected; the surviving record must be returned"
    );
    assert_eq!(result.client_updated_at, 2000);

    let stored = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.epub_cfi.as_deref(), Some("epubcfi(newer)"));
}

#[tokio::test]
async fn upsert_accepts_a_write_with_a_strictly_newer_client_timestamp() {
    // Issue #1362: the mirror image of the rejection case — a genuinely
    // newer client_updated_at must win even though it arrives second.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(older)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1000),
        },
    )
    .await
    .unwrap();

    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(newer)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(2000),
        },
    )
    .await
    .unwrap();

    assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(newer)"));
    assert_eq!(saved.client_updated_at, 2000);
}

#[tokio::test]
async fn upsert_clamps_a_future_client_timestamp_to_server_now() {
    // Issue #1362 AC2: a device with a fast clock must not be able to pin
    // itself as permanently newest by sending a far-future timestamp.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let far_future = now_unix() + 1_000_000;
    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(fast-clock)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(far_future),
        },
    )
    .await
    .unwrap();

    assert!(
        saved.client_updated_at < far_future,
        "a future client_updated_at must be clamped, got {}",
        saved.client_updated_at
    );
    let now = now_unix();
    assert!(
        (now - saved.client_updated_at).abs() <= 5,
        "clamped value should land near server now ({now}), got {}",
        saved.client_updated_at
    );
}

#[tokio::test]
async fn upsert_defaults_missing_client_timestamp_to_server_now() {
    // Issue #1362 AC3: an older client that never sends client_updated_at
    // must still succeed and behave as before — last-write-wins on receipt
    // time, which falls out of defaulting the missing value to server now.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(no-client-ts)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .unwrap();

    assert!(saved.client_updated_at > 0);
    assert_eq!(
        saved.client_updated_at, saved.updated_at,
        "a missing client_updated_at should default to the same server-now stamp as updated_at"
    );
}

#[tokio::test]
async fn recent_progress_orders_by_client_event_time_not_server_receipt_time() {
    // Issue #1362 AC4: replaying a stale queued position (server receives
    // it late, so its raw `updated_at` is the newest of the two) must not
    // move that book to the top of Continue Reading — ordering must follow
    // the client's own event time.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, recently_read) = seed(&pool, "/lib", "Recently Read").await;
    let (_, stale_replay) = seed(&pool, "/lib", "Stale Replay").await;

    // Genuinely read most recently (client_updated_at = 5000), written first.
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: recently_read.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(recent)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(5000),
        },
    )
    .await
    .unwrap();
    // An offline read from a week ago (client_updated_at = 1000), whose
    // write only reaches the server *after* the row above — so its DB
    // `updated_at` (server receipt time) is the larger of the two.
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: stale_replay.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(week-old)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1000),
        },
    )
    .await
    .unwrap();

    let rows = recent_progress(&pool, user, 10).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0].book_uuid, recently_read,
        "client event time must win over server receipt order"
    );
    assert_eq!(rows[1].book_uuid, stale_replay);
}

/// Insert a row directly with a NULL `client_updated_at` — the column has
/// no `NOT NULL` constraint, and rows written by a path other than
/// `upsert_progress` (or a pre-migration row not yet backfilled) can
/// legitimately have one.
async fn seed_null_client_updated_at(pool: &SqlitePool, user_id: i64, uuid: &str, updated_at: i64) {
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, epub_cfi, updated_at, client_updated_at)
         VALUES (?, ?, 'epub', 'epubcfi(null-client-ts)', ?, NULL)",
    )
    .bind(user_id)
    .bind(uuid)
    .bind(updated_at)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn get_progress_coalesces_a_null_client_updated_at_to_receipt_time() {
    // A NULL client_updated_at must not make the SELECT error — it should
    // read back as updated_at, matching pre-#1362 semantics.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_null_client_updated_at(&pool, user, &uuid, 555).await;

    let fetched = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.updated_at, 555);
    assert_eq!(fetched.client_updated_at, 555);
}

#[tokio::test]
async fn recent_progress_coalesces_a_null_client_updated_at_to_receipt_time() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_null_client_updated_at(&pool, user, &uuid, 777).await;

    let rows = recent_progress(&pool, user, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].client_updated_at, 777);
}

#[tokio::test]
async fn upsert_overwrites_a_row_whose_stored_client_updated_at_is_null() {
    // Issue #1362 correctness fix: `WHERE excluded.client_updated_at >=
    // reading_progress.client_updated_at` evaluates to NULL (never true)
    // when the stored side is NULL, which would wedge the row forever.
    // Coalescing the stored side to `updated_at` in that comparison keeps a
    // NULL row updatable, matching pre-migration receipt-time semantics.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_null_client_updated_at(&pool, user, &uuid, 100).await;

    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(overwritten)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(200),
        },
    )
    .await
    .unwrap();

    assert_eq!(
        saved.epub_cfi.as_deref(),
        Some("epubcfi(overwritten)"),
        "a NULL-stored row must still accept a newer write"
    );
    assert_eq!(saved.client_updated_at, 200);
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
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
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
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
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
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
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
async fn get_progress_returns_none_for_unknown_book_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert!(
        get_progress(&pool, user, "no-such-uuid", ProgressFormat::Epub)
            .await
            .unwrap()
            .is_none()
    );
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
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
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

/// Seed an audiobook (books + book_files + parts + chapters) with two 600 s
/// parts and three chapters, returning its uuid.
async fn seed_audiobook(pool: &SqlitePool, uuid: &str) -> i64 {
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/ab', 'ab')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '/ab', 'A')")
            .bind(uuid)
            .bind(lib_id)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'M4B', 'a', 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    for (ordinal, dur) in [(0i64, 600.0f64), (1, 600.0)] {
        sqlx::query(
            "INSERT INTO book_file_parts \
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES (?, ?, 'p', 1, 0, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    for (ordinal, start, dur) in [
        (1i64, 0.0f64, 400.0f64),
        (2, 400.0, 400.0),
        (3, 800.0, 400.0),
    ] {
        sqlx::query(
            "INSERT INTO file_chapters \
                (book_file_id, ordinal, title, start_seconds, duration_seconds) \
             VALUES (?, ?, 'ch', ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(start)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    book_id
}

#[tokio::test]
async fn recent_progress_returns_rows_newest_first_within_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid_a) = seed(&pool, "/lib", "Book A").await;
    let (_, uuid_b) = seed(&pool, "/lib", "Book B").await;
    for uuid in [&uuid_a, &uuid_b] {
        upsert_progress(
            &pool,
            user,
            &ProgressUpdate {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Epub,
                epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
                audio_position_seconds: None,
                progress_percent: None,
                kobo_location: None,
                book_file_id: None,
                client_updated_at: None,
            },
        )
        .await
        .unwrap();
    }
    // upserts land in the same wall-clock second; force a strict order on
    // the column recent_progress actually orders by (client_updated_at,
    // with updated_at as its COALESCE fallback).
    sqlx::query(
        "UPDATE reading_progress SET updated_at = 100, client_updated_at = 100 WHERE book_uuid = ?",
    )
    .bind(&uuid_a)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE reading_progress SET updated_at = 200, client_updated_at = 200 WHERE book_uuid = ?",
    )
    .bind(&uuid_b)
    .execute(&pool)
    .await
    .unwrap();

    let rows = recent_progress(&pool, user, 10).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].book_uuid, uuid_b, "newest row first");
    assert_eq!(rows[1].book_uuid, uuid_a);

    let capped = recent_progress(&pool, user, 1).await.unwrap();
    assert_eq!(capped.len(), 1);
    assert_eq!(capped[0].book_uuid, uuid_b);
}

/// Seed an epub position for `(user, uuid)` so the row shows up on the rail.
async fn seed_epub_position(pool: &SqlitePool, user: i64, uuid: &str) {
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .expect("seed position");
}

async fn set_status(pool: &SqlitePool, user: i64, uuid: &str, status: omnibus_shared::ReadStatus) {
    crate::read_status::set_read_status(
        pool,
        user,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.to_string(),
            status,
        },
    )
    .await
    .expect("set read status");
}

#[tokio::test]
async fn recent_progress_excludes_books_marked_finished() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    assert_eq!(recent_progress(&pool, user, 10).await.unwrap().len(), 1);

    set_status(&pool, user, &uuid, omnibus_shared::ReadStatus::Finished).await;

    assert!(
        recent_progress(&pool, user, 10).await.unwrap().is_empty(),
        "a finished book is not something to continue"
    );
}

#[tokio::test]
async fn recent_progress_excludes_books_marked_unread() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    set_status(&pool, user, &uuid, omnibus_shared::ReadStatus::Unread).await;

    assert!(
        recent_progress(&pool, user, 10).await.unwrap().is_empty(),
        "clearing a book to unread takes it off the rail"
    );
}

#[tokio::test]
async fn recent_progress_keeps_a_book_whose_read_status_row_is_absent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;

    // 0046 reads absence as `unread`; the rail is the deliberate exception.
    // Every status write is best-effort, so a book whose write was lost — or
    // one whose position predates the auto-transition — must still show.
    let rows = recent_progress(&pool, user, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].book_uuid, uuid);
}

#[tokio::test]
async fn recent_progress_keeps_a_book_marked_reading() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    set_status(&pool, user, &uuid, omnibus_shared::ReadStatus::Reading).await;

    let rows = recent_progress(&pool, user, 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].book_uuid, uuid);
}

#[tokio::test]
async fn recent_progress_applies_its_limit_after_the_read_status_filter() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, finished) = seed(&pool, "/lib", "Finished Book").await;
    let (_, active) = seed(&pool, "/lib", "Active Book").await;
    seed_epub_position(&pool, user, &finished).await;
    seed_epub_position(&pool, user, &active).await;
    // The finished book is the *newer* row, so a filter applied after the
    // LIMIT would consume the only slot and hand back an empty rail.
    sqlx::query(
        "UPDATE reading_progress SET updated_at = ?, client_updated_at = ? WHERE book_uuid = ?",
    )
    .bind(100)
    .bind(100)
    .bind(&active)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "UPDATE reading_progress SET updated_at = ?, client_updated_at = ? WHERE book_uuid = ?",
    )
    .bind(200)
    .bind(200)
    .bind(&finished)
    .execute(&pool)
    .await
    .unwrap();
    set_status(&pool, user, &finished, omnibus_shared::ReadStatus::Finished).await;

    let rows = recent_progress(&pool, user, 1).await.unwrap();
    assert_eq!(rows.len(), 1, "the filter runs in SQL, ahead of the LIMIT");
    assert_eq!(rows[0].book_uuid, active);
}

#[tokio::test]
async fn recent_progress_read_status_filter_is_scoped_to_the_reading_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, alice, &uuid).await;
    seed_epub_position(&pool, bob, &uuid).await;
    // Read status is per-user: Bob finishing the book says nothing about
    // whether Alice is still reading it.
    set_status(&pool, bob, &uuid, omnibus_shared::ReadStatus::Finished).await;

    assert_eq!(recent_progress(&pool, alice, 10).await.unwrap().len(), 1);
    assert!(recent_progress(&pool, bob, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn resume_points_enrich_audio_rows_with_duration_and_chapter() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    seed_audiobook(&pool, uuid).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            // 450 s → inside chapter 2 (starts at 400 s).
            audio_position_seconds: Some(450.0),
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points.len(), 1);
    let p = &points[0];
    assert_eq!(p.book.title.as_deref(), Some("A"));
    assert_eq!(p.total_duration_seconds, Some(1200.0));
    assert_eq!(p.chapter_number, Some(2));
    assert_eq!(p.chapter_count, Some(3));
}

/// Attach a second audiobook file to `book_id` — a different narration of
/// the same book: one 3000 s part and two chapters, so its totals can't be
/// confused with the first file's (1200 s / three chapters).
async fn seed_second_audiobook_file(pool: &SqlitePool, book_id: i64) -> i64 {
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal) \
         VALUES (?, 'M4B', 'b', 1, 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts \
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'p', 1, 0, 3000.0)",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
    for (ordinal, start, dur) in [(1i64, 0.0f64, 1500.0f64), (2, 1500.0, 1500.0)] {
        sqlx::query(
            "INSERT INTO file_chapters \
                (book_file_id, ordinal, title, start_seconds, duration_seconds) \
             VALUES (?, ?, 'ch', ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(start)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    file_id
}

#[tokio::test]
async fn upsert_round_trips_the_audio_file_the_position_was_taken_in() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id = seed_audiobook(&pool, uuid).await;
    let second = seed_second_audiobook_file(&pool, book_id).await;

    let saved = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(90.0),
            book_file_id: Some(second),
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(saved.book_file_id, Some(second));

    let fetched = get_progress(&pool, user, uuid, ProgressFormat::Audio)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(fetched.book_file_id, Some(second));
}

#[tokio::test]
async fn resume_points_return_the_audio_file_the_position_was_taken_in() {
    // The reported bug: with two audiobooks on one book, the Continue CTA
    // opened the first file by ordinal at the *other* file's timestamp, and
    // read out the first file's duration and chapter count.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id = seed_audiobook(&pool, uuid).await;
    let second = seed_second_audiobook_file(&pool, book_id).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            // 1600 s → chapter 2 of the second file; past the first file's
            // whole 1200 s duration.
            audio_position_seconds: Some(1600.0),
            book_file_id: Some(second),
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points[0].record.book_file_id, Some(second));
    assert_eq!(points[0].total_duration_seconds, Some(3000.0));
    assert_eq!(points[0].chapter_number, Some(2));
    assert_eq!(points[0].chapter_count, Some(2));
}

#[tokio::test]
async fn resume_points_fall_back_to_the_first_audio_file_for_an_unresolvable_file_id() {
    // `book_files.id` is a soft reference: the reindex diff drops and
    // re-inserts rows, so a stored id can name a file that no longer exists
    // (or one on another book). Resuming must still open a real file.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id = seed_audiobook(&pool, uuid).await;
    let second = seed_second_audiobook_file(&pool, book_id).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(450.0),
            book_file_id: Some(second),
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM book_files WHERE id = ?")
        .bind(second)
        .execute(&pool)
        .await
        .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    let first = crate::hls::resolve_audiobook(&pool, uuid)
        .await
        .unwrap()
        .unwrap()
        .book_file_id;
    assert_eq!(
        points[0].record.book_file_id,
        Some(first),
        "a dead file id must not reach the Continue CTA's ?file_id="
    );
    assert_eq!(points[0].total_duration_seconds, Some(1200.0));
    assert_eq!(points[0].chapter_number, Some(2));
    assert_eq!(points[0].chapter_count, Some(3));
}

#[tokio::test]
async fn resume_points_name_the_default_audio_file_when_the_row_stored_none() {
    // Pre-migration rows and clients that don't track the file still get a
    // concrete id, so the CTA links at the file it will actually open.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    seed_audiobook(&pool, uuid).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(450.0),
            book_file_id: None,
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    let expected = crate::hls::resolve_audiobook(&pool, uuid)
        .await
        .unwrap()
        .unwrap()
        .book_file_id;
    assert_eq!(points[0].record.book_file_id, Some(expected));
}

#[tokio::test]
async fn resume_points_drop_a_file_id_that_resolves_to_nothing() {
    // Every audio file gone (ghosted book) — the CTA must fall back to a
    // bare `/listen/:uuid` rather than carry an id nothing can open.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id = seed_audiobook(&pool, uuid).await;
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.into(),
            format: ProgressFormat::Audio,
            epub_cfi: None,
            audio_position_seconds: Some(450.0),
            book_file_id: Some(1),
            client_updated_at: None,
            progress_percent: None,
            kobo_location: None,
        },
    )
    .await
    .unwrap();
    sqlx::query("DELETE FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points[0].record.book_file_id, None);
    assert_eq!(points[0].total_duration_seconds, None);
}

#[tokio::test]
async fn resume_points_skip_rows_whose_book_is_gone_and_leave_epub_totals_empty() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (ghost_id, ghost_uuid) = seed(&pool, "/lib", "Ghost").await;
    let (_, kept_uuid) = seed(&pool, "/lib", "Kept").await;
    for uuid in [&ghost_uuid, &kept_uuid] {
        upsert_progress(
            &pool,
            user,
            &ProgressUpdate {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Epub,
                epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
                audio_position_seconds: None,
                progress_percent: None,
                kobo_location: None,
                book_file_id: None,
                client_updated_at: None,
            },
        )
        .await
        .unwrap();
    }
    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(ghost_id)
        .execute(&pool)
        .await
        .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points.len(), 1, "ghosted book's row is skipped");
    let p = &points[0];
    assert_eq!(p.record.book_uuid, kept_uuid);
    assert_eq!(p.total_duration_seconds, None);
    assert_eq!(p.chapter_number, None);
    assert_eq!(p.chapter_count, None);
}

#[test]
fn chapter_number_at_tracks_boundaries_and_empty_list() {
    let ch = |start: f64, dur: f64| ChapterInfo {
        ordinal: 1,
        title: "x".into(),
        start_seconds: start,
        duration_seconds: dur,
    };
    assert_eq!(chapter_number_at(&[], 10.0), None);
    let chs = vec![ch(0.0, 400.0), ch(400.0, 400.0)];
    assert_eq!(chapter_number_at(&chs, 0.0), Some(1));
    assert_eq!(chapter_number_at(&chs, 399.9), Some(1));
    assert_eq!(chapter_number_at(&chs, 400.0), Some(2));
    assert_eq!(chapter_number_at(&chs, 9000.0), Some(2));
}

#[tokio::test]
async fn get_progress_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_progress(&pool, 1, "any-uuid", ProgressFormat::Epub)
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)));
}

#[tokio::test]
async fn get_playback_rate_returns_none_when_user_has_no_preference() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    assert!(get_playback_rate(&pool, user, &uuid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn set_playback_rate_round_trips_server_authoritative_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let update = omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 };

    let saved = set_playback_rate(&pool, user, &uuid, &update)
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, uuid);
    assert_eq!(saved.playback_rate, 1.5);
    assert!(saved.updated_at > 0);

    assert_eq!(
        get_playback_rate(&pool, user, &uuid).await.unwrap(),
        Some(saved)
    );
}

#[tokio::test]
async fn playback_rate_is_isolated_per_user_and_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid_a) = seed(&pool, "/lib", "Book A").await;
    let (_, uuid_b) = seed(&pool, "/lib", "Book B").await;

    set_playback_rate(
        &pool,
        alice,
        &uuid_a,
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 },
    )
    .await
    .unwrap();

    assert!(get_playback_rate(&pool, alice, &uuid_b)
        .await
        .unwrap()
        .is_none());
    assert!(get_playback_rate(&pool, bob, &uuid_a)
        .await
        .unwrap()
        .is_none());

    set_playback_rate(
        &pool,
        bob,
        &uuid_a,
        &omnibus_shared::AudiobookPlaybackRateUpdate {
            playback_rate: 2.25,
        },
    )
    .await
    .unwrap();
    assert_eq!(
        get_playback_rate(&pool, alice, &uuid_a)
            .await
            .unwrap()
            .unwrap()
            .playback_rate,
        1.5
    );
    assert_eq!(
        get_playback_rate(&pool, bob, &uuid_a)
            .await
            .unwrap()
            .unwrap()
            .playback_rate,
        2.25
    );
}

#[tokio::test]
async fn set_playback_rate_resolves_merged_uuid_to_canonical_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, canonical_uuid) = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "merged-audio-uuid", book_id, "audio").await;

    let saved = set_playback_rate(
        &pool,
        user,
        "merged-audio-uuid",
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.8 },
    )
    .await
    .unwrap();
    assert_eq!(saved.book_uuid, canonical_uuid);
    assert_eq!(
        get_playback_rate(&pool, user, "merged-audio-uuid")
            .await
            .unwrap(),
        Some(saved)
    );
}

#[tokio::test]
async fn set_playback_rate_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let err = set_playback_rate(
        &pool,
        user,
        "no-such-book",
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ProgressError::BookNotFound));
}

#[tokio::test]
async fn get_playback_rate_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let err = get_playback_rate(&pool, user, "no-such-book")
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::BookNotFound));
}

#[tokio::test]
async fn playback_rate_migration_rejects_out_of_range_values() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    let err = sqlx::query(
        "INSERT INTO audiobook_playback_preferences
            (user_id, book_uuid, playback_rate)
         VALUES (?, ?, 3.5)",
    )
    .bind(user)
    .bind(uuid)
    .execute(&pool)
    .await
    .unwrap_err();
    assert!(matches!(err, sqlx::Error::Database(_)));
}

#[tokio::test]
async fn playback_rate_survives_hard_delete_of_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
    set_playback_rate(
        &pool,
        user,
        &uuid,
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 },
    )
    .await
    .unwrap();

    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM audiobook_playback_preferences
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn get_playback_rate_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;

    let err = get_playback_rate(&pool, 1, "any-uuid").await.unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)));
}

#[tokio::test]
async fn set_playback_rate_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;

    let err = set_playback_rate(
        &pool,
        1,
        "any-uuid",
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)));
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

#[tokio::test]
async fn upsert_stores_a_blank_cfi_as_null_never_as_an_anchor() {
    // Defense in depth: `validate` rejects a blank CFI at the API boundary,
    // and the bind normalizes it to NULL so an internal caller that skipped
    // validation replaces the position with an honest "no CFI" rather than
    // storing whitespace as an anchor.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let cfi = "epubcfi(/6/4!/4/2/1:0)";
    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some(cfi.into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            client_updated_at: Some(100),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let after = upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("   ".into()),
            audio_position_seconds: None,
            progress_percent: Some(70),
            kobo_location: None,
            client_updated_at: Some(200),
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        after.epub_cfi, None,
        "whitespace must not store as an anchor"
    );
    assert_eq!(after.progress_percent, Some(70));
    let _ = cfi;
}

#[tokio::test]
async fn audio_rows_cannot_carry_the_epub_only_position_columns() {
    // The row CHECK enforces the cross-format invariant independently of the
    // API validators, so an internal caller can't persist a mixed row.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    let err = sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, audio_position_seconds, progress_percent, updated_at)
         VALUES (?, ?, 'audio', 12.0, 50, 1)",
    )
    .bind(user)
    .bind(&uuid)
    .execute(&pool)
    .await
    .expect_err("an audio row with a percent must violate the CHECK");
    assert!(
        err.to_string().to_lowercase().contains("constraint"),
        "got: {err}"
    );
}
