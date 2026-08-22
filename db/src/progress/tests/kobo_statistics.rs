//! `set_kobo_statistics_tx`: storing the mirrored Kobo `Statistics` block on
//! an existing EPUB position row, its staleness rules and device-clock clamp,
//! and the guarantee that a later position write preserves it.

use omnibus_shared::ProgressUpdate;
use sqlx::{Row, SqlitePool};

use crate::init_db;

use super::super::*;
use super::{seed, seed_epub_position, seed_user};

/// The stored `(spent, remaining, stamp)` triple for an epub row.
async fn stored_statistics(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
) -> (Option<i64>, Option<i64>, Option<i64>) {
    let row = sqlx::query(
        "SELECT kobo_spent_reading_minutes, kobo_remaining_time_minutes,
                kobo_statistics_updated_at
           FROM reading_progress WHERE user_id = ? AND book_uuid = ? AND format = 'epub'",
    )
    .bind(user)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .expect("position row");
    (
        row.try_get(0).unwrap(),
        row.try_get(1).unwrap(),
        row.try_get(2).unwrap(),
    )
}

async fn set_stats(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    stats: &KoboStatistics,
) -> Result<bool, ProgressError> {
    let mut tx = pool.begin().await.unwrap();
    let updated = set_kobo_statistics_tx(&mut tx, user, uuid, stats).await?;
    tx.commit().await.unwrap();
    Ok(updated)
}

#[tokio::test]
async fn set_kobo_statistics_tx_stores_the_block_on_an_existing_epub_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;

    let stats = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: Some(75),
        updated_at: Some(1_700_000_000),
    };
    assert!(set_stats(&pool, user, &uuid, &stats).await.unwrap());
    assert_eq!(
        stored_statistics(&pool, user, &uuid).await,
        (Some(340), Some(75), Some(1_700_000_000))
    );
}

#[tokio::test]
async fn set_kobo_statistics_tx_does_not_create_a_row_for_a_book_with_no_position() {
    // Statistics annotate a position; a stats-only row can't satisfy the epub
    // CHECK, and would surface position-less on the Continue-reading rail.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    let stats = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: None,
        updated_at: Some(1_700_000_000),
    };
    assert!(!set_stats(&pool, user, &uuid, &stats).await.unwrap());
    assert!(get_progress(&pool, user, &uuid, ProgressFormat::Epub)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn set_kobo_statistics_tx_rejects_a_block_older_than_the_stored_one() {
    // Two Kobos on one account: the older device's report must not overwrite
    // the newer one's totals and then be echoed back to it as truth.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    let newer = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: Some(75),
        updated_at: Some(1_700_001_000),
    };
    assert!(set_stats(&pool, user, &uuid, &newer).await.unwrap());

    let older = KoboStatistics {
        spent_reading_minutes: Some(12),
        remaining_time_minutes: Some(900),
        updated_at: Some(1_700_000_000),
    };
    assert!(!set_stats(&pool, user, &uuid, &older).await.unwrap());
    assert_eq!(
        stored_statistics(&pool, user, &uuid).await,
        (Some(340), Some(75), Some(1_700_001_000))
    );
}

#[tokio::test]
async fn set_kobo_statistics_tx_rejects_an_unstamped_block_over_a_stamped_one() {
    // An unstamped block can't be shown to be newer, so it may only take an
    // empty slot — it can't clear a clock the echo path depends on.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    let stamped = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: None,
        updated_at: Some(1_700_001_000),
    };
    assert!(set_stats(&pool, user, &uuid, &stamped).await.unwrap());

    let unstamped = KoboStatistics {
        spent_reading_minutes: Some(12),
        remaining_time_minutes: None,
        updated_at: None,
    };
    assert!(!set_stats(&pool, user, &uuid, &unstamped).await.unwrap());
    assert_eq!(
        stored_statistics(&pool, user, &uuid).await,
        (Some(340), None, Some(1_700_001_000))
    );
}

#[tokio::test]
async fn set_kobo_statistics_tx_stores_an_unstamped_block_in_an_empty_slot() {
    // No clock means sync-out omits `LastModified` and the device drops the
    // block — safe. Refusing the write entirely would lose data for nothing.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;

    let stats = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: None,
        updated_at: None,
    };
    assert!(set_stats(&pool, user, &uuid, &stats).await.unwrap());
    assert_eq!(
        stored_statistics(&pool, user, &uuid).await,
        (Some(340), None, None)
    );
}

#[tokio::test]
async fn set_kobo_statistics_tx_clamps_a_device_clock_from_the_future() {
    // An unclamped future stamp would lock the row out of every later update
    // and win arbitration on the device forever.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    let now: i64 = sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER)")
        .fetch_one(&pool)
        .await
        .unwrap();

    let stats = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: None,
        updated_at: Some(now + 86_400),
    };
    assert!(set_stats(&pool, user, &uuid, &stats).await.unwrap());
    let (_, _, stored) = stored_statistics(&pool, user, &uuid).await;
    let stored = stored.expect("stamp stored");
    assert!(stored <= now + 2, "expected a clamp to now, got {stored}");
}

#[tokio::test]
async fn set_kobo_statistics_tx_returns_book_not_found_for_an_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let err = set_stats(&pool, user, "no-such-uuid", &KoboStatistics::default())
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::BookNotFound));
}

#[tokio::test]
async fn set_kobo_statistics_tx_propagates_db_error_when_the_table_is_gone() {
    // The write can't be reached through a closed pool (no transaction to
    // hand it), so the table is dropped inside the transaction instead.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE reading_progress")
        .execute(&mut *tx)
        .await
        .unwrap();

    let err = set_kobo_statistics_tx(&mut tx, user, &uuid, &KoboStatistics::default())
        .await
        .unwrap_err();
    assert!(matches!(err, ProgressError::Sqlx(_)));
}

#[tokio::test]
async fn upsert_progress_preserves_a_stored_statistics_block() {
    // A position write is a statement about where the reader is, not about
    // how long they've read — clearing the counters would make the next
    // sync-out drop a block the device is entitled to get back.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    seed_epub_position(&pool, user, &uuid).await;
    let stats = KoboStatistics {
        spent_reading_minutes: Some(340),
        remaining_time_minutes: Some(75),
        updated_at: Some(1_700_000_000),
    };
    assert!(set_stats(&pool, user, &uuid, &stats).await.unwrap());

    upsert_progress(
        &pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/8/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: Some(61),
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(
        stored_statistics(&pool, user, &uuid).await,
        (Some(340), Some(75), Some(1_700_000_000))
    );
}

#[test]
fn kobo_statistics_is_empty_only_when_both_counters_are_absent() {
    assert!(KoboStatistics::default().is_empty());
    assert!(KoboStatistics {
        spent_reading_minutes: None,
        remaining_time_minutes: None,
        updated_at: Some(1_700_000_000),
    }
    .is_empty());
    assert!(!KoboStatistics {
        spent_reading_minutes: Some(0),
        remaining_time_minutes: None,
        updated_at: None,
    }
    .is_empty());
}
