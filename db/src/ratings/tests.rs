//! Unit tests for the `ratings` module: `set_rating` round-trip (incl.
//! half-star storage), change-overwrites, per-user isolation, `BookNotFound`,
//! `get_rating` empty-state, `delete_rating` clear + absent no-op, and
//! merged-uuid canonical resolution.

use omnibus_shared::EbookMetadata;

use super::*;
use crate::{init_db, replace_books};

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

/// Map a merged/auto-attached `uuid` onto an existing `book_id` the way the
/// merge transaction does, so the canonical-resolution path has a row to
/// resolve through the `merged_uuids` UNION fallback.
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

fn update(uuid: &str, stars: f32) -> RatingUpdate {
    RatingUpdate {
        book_uuid: uuid.to_string(),
        stars,
    }
}

#[tokio::test]
async fn set_rating_round_trips_half_star_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    let saved = set_rating(&pool, user, &update(&uuid, 4.5)).await.unwrap();
    assert_eq!(saved.book_uuid, uuid);
    assert_eq!(saved.stars, 4.5);

    // Stored as an exact half-star count (9 = 4.5★).
    let half: i64 = sqlx::query_scalar("SELECT half_stars FROM user_ratings WHERE book_uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(half, 9);

    let fetched = get_rating(&pool, user, &uuid).await.unwrap().unwrap();
    assert_eq!(fetched.stars, 4.5);
    assert_eq!(fetched.updated_at, saved.updated_at);
}

#[tokio::test]
async fn set_rating_overwrites_existing_value() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    set_rating(&pool, user, &update(&uuid, 2.5)).await.unwrap();
    let updated = set_rating(&pool, user, &update(&uuid, 5.0)).await.unwrap();
    assert_eq!(updated.stars, 5.0);

    // Still exactly one row for this (user, book).
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM user_ratings WHERE book_uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "upsert keeps a single row");
}

#[tokio::test]
async fn set_rating_is_isolated_per_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    set_rating(&pool, alice, &update(&uuid, 1.0)).await.unwrap();
    set_rating(&pool, bob, &update(&uuid, 5.0)).await.unwrap();

    assert_eq!(
        get_rating(&pool, alice, &uuid)
            .await
            .unwrap()
            .unwrap()
            .stars,
        1.0
    );
    assert_eq!(
        get_rating(&pool, bob, &uuid).await.unwrap().unwrap().stars,
        5.0
    );
}

#[tokio::test]
async fn set_rating_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let err = set_rating(&pool, user, &update("no-such-book", 3.0))
        .await
        .unwrap_err();
    assert!(matches!(err, RatingError::BookNotFound));
}

#[tokio::test]
async fn get_rating_returns_none_when_unrated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    assert!(get_rating(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn get_rating_returns_none_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert!(get_rating(&pool, user, "no-such-book")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn delete_rating_clears_the_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    set_rating(&pool, user, &update(&uuid, 3.0)).await.unwrap();

    delete_rating(&pool, user, &uuid).await.unwrap();
    assert!(get_rating(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_rating_is_a_noop_when_absent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;

    // No rating yet, and an unknown uuid — neither errors.
    delete_rating(&pool, user, &uuid).await.unwrap();
    delete_rating(&pool, user, "no-such-book").await.unwrap();
}

#[tokio::test]
async fn set_rating_resolves_merged_uuid_to_surviving_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (book_id, canonical) = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "attached-uuid", book_id, "EPUB").await;

    // Rate via the merged (attach-ledger) uuid; it must store under the
    // surviving book's canonical uuid.
    let saved = set_rating(&pool, user, &update("attached-uuid", 4.5))
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, canonical);

    // Readable via both the merged uuid and the canonical uuid.
    assert_eq!(
        get_rating(&pool, user, "attached-uuid")
            .await
            .unwrap()
            .unwrap()
            .stars,
        4.5
    );
    assert_eq!(
        get_rating(&pool, user, &canonical)
            .await
            .unwrap()
            .unwrap()
            .stars,
        4.5
    );
}

#[tokio::test]
async fn set_rating_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    pool.close().await;

    let err = set_rating(&pool, user, &update(&uuid, 3.0))
        .await
        .unwrap_err();
    assert!(matches!(err, RatingError::Sqlx(_)));
}
