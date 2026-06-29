//! Unit tests for the `bookmarks` CRUD module — create round-trip,
//! BookNotFound / NotFound variants, list isolation, title updates, and
//! delete behaviour.

use super::*;
use crate::{init_db, replace_books};
use omnibus_shared::EbookMetadata;

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

fn input(uuid: &str, position: &str, title: Option<&str>) -> CreateBookmark {
    CreateBookmark {
        book_uuid: uuid.into(),
        position: position.into(),
        title: title.map(str::to_string),
    }
}

#[tokio::test]
async fn create_bookmark_round_trips_fields() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let b = create_bookmark(&pool, user, &input(&uuid, "1234.5", Some("A mark")))
        .await
        .unwrap();
    assert_eq!(b.book_uuid, uuid);
    assert_eq!(b.position, "1234.5");
    assert_eq!(b.title.as_deref(), Some("A mark"));
    assert!(b.created_at > 0);
}

#[tokio::test]
async fn create_bookmark_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let err = create_bookmark(&pool, user, &input("no-such-uuid", "0", None))
        .await
        .unwrap_err();
    assert!(matches!(err, BookmarkError::BookNotFound));
}

#[tokio::test]
async fn list_bookmarks_returns_empty_when_none_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let list = list_bookmarks(&pool, user, &uuid).await.unwrap();
    assert!(list.is_empty());
}

#[tokio::test]
async fn list_bookmarks_isolates_by_user_and_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid_a) = seed(&pool, "/lib-a", "Book A").await;
    let (_, uuid_b) = seed(&pool, "/lib-b", "Book B").await;

    create_bookmark(&pool, alice, &input(&uuid_a, "10", None))
        .await
        .unwrap();
    create_bookmark(&pool, alice, &input(&uuid_b, "20", None))
        .await
        .unwrap();
    create_bookmark(&pool, bob, &input(&uuid_a, "30", None))
        .await
        .unwrap();

    assert_eq!(
        list_bookmarks(&pool, alice, &uuid_a).await.unwrap().len(),
        1
    );
    assert_eq!(
        list_bookmarks(&pool, alice, &uuid_b).await.unwrap().len(),
        1
    );
    assert_eq!(list_bookmarks(&pool, bob, &uuid_a).await.unwrap().len(), 1);
    assert!(list_bookmarks(&pool, bob, &uuid_b)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn update_bookmark_sets_and_clears_title() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let b = create_bookmark(&pool, user, &input(&uuid, "5", None))
        .await
        .unwrap();
    assert!(b.title.is_none());

    update_bookmark(&pool, user, b.id, Some("come back here"))
        .await
        .unwrap();
    let list = list_bookmarks(&pool, user, &uuid).await.unwrap();
    assert_eq!(list[0].title.as_deref(), Some("come back here"));

    update_bookmark(&pool, user, b.id, None).await.unwrap();
    let list = list_bookmarks(&pool, user, &uuid).await.unwrap();
    assert!(list[0].title.is_none());
}

#[tokio::test]
async fn update_bookmark_returns_not_found_for_other_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let b = create_bookmark(&pool, alice, &input(&uuid, "5", None))
        .await
        .unwrap();
    let err = update_bookmark(&pool, bob, b.id, Some("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, BookmarkError::NotFound));
}

#[tokio::test]
async fn delete_bookmark_removes_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let b = create_bookmark(&pool, user, &input(&uuid, "5", None))
        .await
        .unwrap();
    delete_bookmark(&pool, user, b.id).await.unwrap();
    assert!(list_bookmarks(&pool, user, &uuid).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_bookmark_returns_not_found_for_other_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let (_, uuid) = seed(&pool, "/lib", "Book A").await;
    let b = create_bookmark(&pool, alice, &input(&uuid, "5", None))
        .await
        .unwrap();
    let err = delete_bookmark(&pool, bob, b.id).await.unwrap_err();
    assert!(matches!(err, BookmarkError::NotFound));
}
