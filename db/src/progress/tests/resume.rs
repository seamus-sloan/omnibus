//! The resume-card read path: `recent_progress` ordering, limit, and
//! read-status filtering, plus the duration / chapter / audio-file
//! enrichment `resume_points` layers on top.

use omnibus_shared::{ChapterInfo, ProgressUpdate};
use sqlx::SqlitePool;

use crate::init_db;

use super::super::resume::chapter_number_at;
use super::super::*;
use super::{
    seed, seed_audiobook, seed_epub_position, seed_null_client_updated_at,
    seed_second_audiobook_file, seed_user,
};

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
    // No saved preference: the resume surfaces treat this as 1x.
    assert_eq!(p.playback_rate, None);
}

#[tokio::test]
async fn resume_points_carry_the_saved_playback_rate_for_audio_rows() {
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
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .unwrap();
    set_playback_rate(
        &pool,
        user,
        uuid,
        &omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 2.0 },
    )
    .await
    .unwrap();

    let points = resume_points(&pool, user, 5).await.unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].playback_rate, Some(2.0));
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
