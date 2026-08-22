//! `upsert_progress` most-recent-wins conflict resolution: round-tripping a
//! stored position, the mutually exclusive EPUB/Kobo anchor columns, and the
//! client-event-time rejection / acceptance / clamping / NULL rules.

use omnibus_shared::ProgressUpdate;

use crate::{auth::now_unix, init_db};

use super::super::*;
use super::{seed, seed_null_client_updated_at, seed_user};

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
