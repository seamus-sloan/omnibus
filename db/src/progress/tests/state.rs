//! Per-book progress state outside the upsert conflict path: the
//! server-derived Kobo location attach, the plain `get_progress` getter, and
//! the durability of a progress row across a hard book delete.

use omnibus_shared::ProgressUpdate;

use crate::init_db;

use super::super::*;
use super::{seed, seed_null_client_updated_at, seed_user};

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

#[tokio::test]
async fn get_progress_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_progress(&pool, 1, "any-uuid", ProgressFormat::Epub)
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)));
}
