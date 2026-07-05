//! Unit tests for the `journals` module: create round-trip + markdown render,
//! `BookNotFound`, the public all-users list ordering, owner-scoped
//! update/delete (with non-owner `NotFound`), and merged-uuid canonical
//! resolution.

use super::*;
use crate::{init_db, replace_books};
use omnibus_shared::{CreateJournalEntry, EbookMetadata, UpdateJournalEntry};

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> String {
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
    books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap()
        .unique_identifier
        .unwrap()
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

async fn seed_merged_uuid(pool: &SqlitePool, uuid: &str, book_uuid: &str, format: &str) {
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, ?, '/lib')",
    )
    .bind(uuid)
    .bind(book_id)
    .bind(format)
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

fn create(uuid: &str, body: &str, progress: Option<u8>) -> CreateJournalEntry {
    CreateJournalEntry {
        book_uuid: uuid.to_string(),
        body_md: body.to_string(),
        progress,
    }
}

#[tokio::test]
async fn create_round_trips_and_renders_markdown() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;

    let saved = create_journal_entry(&pool, user, &create(&uuid, "**loved** it", Some(42)))
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, uuid);
    assert_eq!(saved.author_id, user);
    assert_eq!(saved.author_name, "alice");
    assert_eq!(saved.body_md, "**loved** it");
    assert!(saved.body_html.contains("<strong>loved</strong>"));
    assert_eq!(saved.progress, Some(42));
}

#[tokio::test]
async fn create_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let err = create_journal_entry(&pool, user, &create("no-such-book", "x", None))
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::BookNotFound));
}

#[tokio::test]
async fn list_returns_all_users_entries_newest_first() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;

    create_journal_entry(&pool, alice, &create(&uuid, "alice first", None))
        .await
        .unwrap();
    create_journal_entry(&pool, bob, &create(&uuid, "bob second", None))
        .await
        .unwrap();

    let entries = list_journal_entries(&pool, &uuid).await.unwrap();
    assert_eq!(entries.len(), 2, "every user's entries are public");
    // Newest first (created_at DESC, id DESC) — bob inserted last.
    assert_eq!(entries[0].author_name, "bob");
    assert_eq!(entries[0].body_md, "bob second");
    assert_eq!(entries[1].author_name, "alice");
}

#[tokio::test]
async fn list_returns_empty_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(list_journal_entries(&pool, "no-such-book")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn update_by_owner_changes_body() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, user, &create(&uuid, "draft", None))
        .await
        .unwrap();

    let updated = update_journal_entry(
        &pool,
        user,
        entry.id,
        &UpdateJournalEntry {
            body_md: "*revised*".into(),
            progress: Some(90),
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.body_md, "*revised*");
    assert!(updated.body_html.contains("<em>revised</em>"));
    assert_eq!(updated.progress, Some(90));
}

#[tokio::test]
async fn update_by_non_owner_returns_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, alice, &create(&uuid, "mine", None))
        .await
        .unwrap();

    let err = update_journal_entry(
        &pool,
        bob,
        entry.id,
        &UpdateJournalEntry {
            body_md: "hijack".into(),
            progress: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, JournalError::NotFound));
    // Untouched.
    let entries = list_journal_entries(&pool, &uuid).await.unwrap();
    assert_eq!(entries[0].body_md, "mine");
}

#[tokio::test]
async fn delete_by_owner_removes_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, user, &create(&uuid, "bye", None))
        .await
        .unwrap();

    delete_journal_entry(&pool, user, entry.id).await.unwrap();
    assert!(list_journal_entries(&pool, &uuid).await.unwrap().is_empty());
}

#[tokio::test]
async fn delete_by_non_owner_returns_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, alice, &create(&uuid, "mine", None))
        .await
        .unwrap();

    let err = delete_journal_entry(&pool, bob, entry.id)
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::NotFound));
    assert_eq!(list_journal_entries(&pool, &uuid).await.unwrap().len(), 1);
}

#[tokio::test]
async fn create_resolves_merged_uuid_to_surviving_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let canonical = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "attached-uuid", &canonical, "EPUB").await;

    let saved = create_journal_entry(&pool, user, &create("attached-uuid", "note", None))
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, canonical);
    // Readable via both the merged and the canonical uuid.
    assert_eq!(
        list_journal_entries(&pool, "attached-uuid")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_journal_entries(&pool, &canonical).await.unwrap().len(),
        1
    );
}

/// Bulk-insert `count` entry rows for `book_uuid` without going through
/// `create_journal_entry` — too slow at over-cap row counts.
async fn seed_entries_raw(pool: &SqlitePool, user_id: i64, book_uuid: &str, count: i64) {
    sqlx::query(
        r#"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO journal_entries (user_id, book_uuid, body_md, created_at)
        SELECT ?, ?, 'entry ' || i, i FROM n
        "#,
    )
    .bind(count)
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_journal_entries_caps_response_at_hard_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let over_cap = LIST_JOURNAL_ENTRIES_LIMIT + 500;
    seed_entries_raw(&pool, user, &uuid, over_cap).await;

    let list = list_journal_entries(&pool, &uuid).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_JOURNAL_ENTRIES_LIMIT,
        "list_journal_entries must not return more than LIST_JOURNAL_ENTRIES_LIMIT rows",
    );
}

#[tokio::test]
async fn create_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    pool.close().await;

    let err = create_journal_entry(&pool, user, &create(&uuid, "x", None))
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::Sqlx(_)));
}
