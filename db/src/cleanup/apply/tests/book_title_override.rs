//! `apply_book_title_override`: the override write never touches
//! `books.title` directly, other override fields survive the round trip,
//! a failed `cleanup_log` insert rolls back the override write in the same
//! transaction, and two concurrent `undo()` calls on the same rename race
//! to exactly one success.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use super::{count_rows, insert_book, new_pool, seed_root, seed_user, tally_race_outcomes, undo};
use crate::metadata_overrides::{get_metadata_overrides, upsert_metadata_overrides};

#[tokio::test]
async fn apply_book_title_override_sets_override_without_touching_books_title_and_undo_deletes_it()
{
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Maas, Sarah J - Throne of Glass").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();

    let log_id = apply_book_title_override(&pool, &uuid, "Throne of Glass", None, user)
        .await
        .unwrap();

    let stored_title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        stored_title, "Maas, Sarah J - Throne of Glass",
        "books.title must never be touched directly"
    );
    let (ov, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(ov.title.as_deref(), Some("Throne of Glass"));

    undo(&pool, log_id).await.unwrap();

    assert!(get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn apply_book_title_override_preserves_other_override_fields_and_undo_restores_them() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Cruft Title (Annotated)").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let existing = MetadataOverrides {
        description: Some("An existing description.".to_string()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &existing, false, user)
        .await
        .unwrap();

    let log_id = apply_book_title_override(&pool, &uuid, "Cruft Title", None, user)
        .await
        .unwrap();

    let (ov, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(ov.title.as_deref(), Some("Cruft Title"));
    assert_eq!(ov.description.as_deref(), Some("An existing description."));

    undo(&pool, log_id).await.unwrap();

    let (restored, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(restored.title, None);
    assert_eq!(
        restored.description.as_deref(),
        Some("An existing description.")
    );
}

/// Composed-transaction proof: force the `cleanup_log` INSERT to fail
/// *after* the override write has already run, by naming a `suggestion_id`
/// with no matching `dedup_suggestions` row (`cleanup_log.suggestion_id`
/// carries an FK, and the pool enables `PRAGMA foreign_keys = ON`). Before
/// this fix, the override write committed on its own `BEGIN IMMEDIATE` and
/// only the log INSERT failed, leaving an unloggable rename in place. Now
/// both writes share one transaction, so the FK failure rolls the override
/// write back too — the book keeps its pre-call state and nothing is
/// left half-applied.
#[tokio::test]
async fn apply_book_title_override_rolls_back_the_override_write_when_the_log_insert_fails() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Maas, Sarah J - Throne of Glass").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();

    let bogus_suggestion_id = 999_999;
    let err = apply_book_title_override(
        &pool,
        &uuid,
        "Throne of Glass",
        Some(bogus_suggestion_id),
        user,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, CleanupApplyError::Db(_)));

    assert!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "the override write must not survive a failed cleanup_log INSERT \
         in the same transaction"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM cleanup_log").await,
        0,
        "the failed INSERT itself must leave no row behind either"
    );
}

/// Same race as the merge-authors concurrent-undo test, against the
/// `BookTitle`/`Rename` log kind — the path that used to call `mark_undone`
/// (pool-level, its own statement) only *after* the `metadata_overrides`
/// restore, and outside any shared transaction with it. Now both the claim
/// and the restore run inside one transaction via
/// `upsert_one_in_tx`/`delete_one_in_tx`, so the same claim-then-mutate
/// guarantee holds here too.
#[tokio::test]
async fn undo_concurrent_calls_on_a_rename_race_to_exactly_one_success() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Cruft Title (Annotated)").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();

    let log_id = apply_book_title_override(&pool, &uuid, "Cruft Title", None, user)
        .await
        .unwrap();

    let barrier = Arc::new(Barrier::new(2));
    let (pool_a, barrier_a) = (pool.clone(), barrier.clone());
    let (pool_b, barrier_b) = (pool.clone(), barrier.clone());
    let task_a = tokio::spawn(async move {
        barrier_a.wait().await;
        undo(&pool_a, log_id).await
    });
    let task_b = tokio::spawn(async move {
        barrier_b.wait().await;
        undo(&pool_b, log_id).await
    });
    let results = [task_a.await.unwrap(), task_b.await.unwrap()];

    let (ok, already_undone) = tally_race_outcomes(&results);
    assert_eq!(ok, 1, "exactly one racing undo() call must succeed");
    assert_eq!(already_undone, 1, "the other must see AlreadyUndone");

    assert!(get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}
