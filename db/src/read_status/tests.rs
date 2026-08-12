//! Unit tests for the `read_status` module: `set_read_status` round-trip and
//! overwrite, `finished_at` lifecycle (set on finish, preserved on re-affirm,
//! cleared on downgrade), per-user isolation, `BookNotFound`, `get_read_status`
//! empty-state, and merged-uuid canonical resolution.

use omnibus_shared::{EbookMetadata, ReadStatus, SetReadStatus};

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

fn set(uuid: &str, status: ReadStatus) -> SetReadStatus {
    SetReadStatus {
        book_uuid: uuid.to_string(),
        status,
    }
}

#[tokio::test]
async fn set_read_status_round_trips_the_status() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;

    let rec = set_read_status(&pool, user, &set(&uuid, ReadStatus::Reading))
        .await
        .unwrap();
    assert_eq!(rec.status, ReadStatus::Reading);
    assert_eq!(rec.book_uuid, uuid);
    assert!(rec.finished_at.is_none());

    let got = get_read_status(&pool, user, &uuid).await.unwrap().unwrap();
    assert_eq!(got.status, ReadStatus::Reading);
}

#[tokio::test]
async fn set_read_status_overwrites_existing_status() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;

    set_read_status(&pool, user, &set(&uuid, ReadStatus::Reading))
        .await
        .unwrap();
    let rec = set_read_status(&pool, user, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    assert_eq!(rec.status, ReadStatus::Finished);

    // Only one row exists for the pair — the second write updated in place.
    let count: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_read_status WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn finishing_stamps_finished_at() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;

    let rec = set_read_status(&pool, user, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    assert!(rec.finished_at.is_some());
}

#[tokio::test]
async fn re_affirming_finished_preserves_original_finished_at() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;

    let first = set_read_status(&pool, user, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    // Backdate the stored completion so a re-affirm would visibly change it if
    // the COALESCE branch were wrong.
    sqlx::query(
        "UPDATE book_read_status SET finished_at = 100 WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user)
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();
    let again = set_read_status(&pool, user, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    assert_eq!(again.finished_at, Some(100));
    let _ = first;
}

#[tokio::test]
async fn downgrading_from_finished_clears_finished_at() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;

    set_read_status(&pool, user, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    let rec = set_read_status(&pool, user, &set(&uuid, ReadStatus::Unread))
        .await
        .unwrap();
    assert_eq!(rec.status, ReadStatus::Unread);
    assert!(rec.finished_at.is_none());
}

#[tokio::test]
async fn read_status_is_isolated_per_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let a = seed_user(&pool, "alice").await;
    let b = seed_user(&pool, "bob").await;

    set_read_status(&pool, a, &set(&uuid, ReadStatus::Finished))
        .await
        .unwrap();
    assert!(get_read_status(&pool, b, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn set_read_status_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let err = set_read_status(&pool, user, &set("nope", ReadStatus::Reading))
        .await
        .unwrap_err();
    assert!(matches!(err, ReadStatusError::BookNotFound));
}

#[tokio::test]
async fn get_read_status_returns_none_when_unset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (_, uuid) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;
    assert!(get_read_status(&pool, user, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn get_read_status_returns_none_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    assert!(get_read_status(&pool, user, "nope")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn set_read_status_resolves_merged_uuid_to_surviving_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (book_id, canonical) = seed(&pool, "/lib", "Dune").await;
    let user = seed_user(&pool, "reader").await;
    seed_merged_uuid(&pool, "legacy-uuid", book_id, "epub").await;

    let rec = set_read_status(&pool, user, &set("legacy-uuid", ReadStatus::Finished))
        .await
        .unwrap();
    // Stored + returned under the surviving book's canonical uuid.
    assert_eq!(rec.book_uuid, canonical);
    let got = get_read_status(&pool, user, &canonical)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(got.status, ReadStatus::Finished);
}

/// #1862: `set_read_status_tx` reads (resolves the canonical uuid) before it
/// writes, the same shape that made `progress::upsert_progress_tx` vulnerable
/// to `SQLITE_BUSY_SNAPSHOT` under a plain `pool.begin()`. Several rounds of
/// overlapping writers — on the same row and on distinct rows — give that
/// race a real chance to have fired without the `BEGIN IMMEDIATE` fix.
#[tokio::test]
async fn set_read_status_succeeds_for_many_concurrent_writers_on_one_pool() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let (_, shared_uuid) = seed(&pool, "/lib", "Shared Book").await;
    let mut solo_uuids = Vec::new();
    for i in 0..4 {
        let (_, uuid) = seed(&pool, "/lib", &format!("Solo Status Book {i}")).await;
        solo_uuids.push(uuid);
    }

    for _round in 0..5 {
        let mut handles = Vec::new();
        for book_uuid in std::iter::once(shared_uuid.clone()).chain(solo_uuids.iter().cloned()) {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                set_read_status(&pool, user, &set(&book_uuid, ReadStatus::Reading)).await
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("writer task panicked")
                .expect("concurrent set_read_status must not surface a database error");
        }
    }
}
