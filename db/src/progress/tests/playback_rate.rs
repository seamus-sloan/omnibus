//! `get_playback_rate` / `set_playback_rate`: the server-authoritative
//! audiobook speed preference, its per-user/book isolation, merged-uuid
//! resolution, range constraint, and durability across a hard book delete.

use crate::init_db;

use super::super::*;
use super::{seed, seed_merged_uuid, seed_user};

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
