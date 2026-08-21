//! `undo`'s own error paths: an unknown log id, a second undo of an
//! already-undone entry, and a `Rename` log row with no `applied_by`
//! actor to attribute the restore to.

use super::super::*;
use super::{insert_author, insert_book, new_pool, seed_root, undo};

#[tokio::test]
async fn undo_returns_log_not_found_for_an_unknown_id() {
    let pool = new_pool().await;
    let err = undo(&pool, 9999).await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::LogNotFound(9999)));
}

#[tokio::test]
async fn undo_returns_already_undone_on_a_second_call() {
    let pool = new_pool().await;
    let author = insert_author(&pool, "Solo Author", None).await;
    let log_id = apply_delete_author(&pool, author, None, None)
        .await
        .unwrap();

    undo(&pool, log_id).await.unwrap();
    let err = undo(&pool, log_id).await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::AlreadyUndone));
}

#[tokio::test]
async fn undo_returns_missing_actor_for_a_rename_log_row_with_no_applied_by() {
    // `apply_book_title_override` always writes a real `applied_by` (it
    // takes `i64`, not `Option<i64>`), so a `Rename` row with a restorable
    // `previous_overrides` blob and no actor is unreachable through any
    // existing caller — construct it directly to prove `undo` still fails
    // this case safely rather than losing whom to attribute the restore to.
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Some Title").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let snapshot = RenameSnapshot {
        book_uuid: uuid,
        previous_overrides: Some(
            serde_json::to_string(&omnibus_shared::MetadataOverrides::default()).unwrap(),
        ),
        previous_has_cover_override: false,
    };
    let log_id: i64 = sqlx::query_scalar(
        "INSERT INTO cleanup_log (suggestion_id, kind, action, snapshot_json, applied_by)
         VALUES (NULL, ?, ?, ?, NULL) RETURNING id",
    )
    .bind(omnibus_shared::CleanupKind::BookTitle.as_str())
    .bind(omnibus_shared::CleanupAction::Rename.as_str())
    .bind(serde_json::to_string(&snapshot).unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let err = undo(&pool, log_id).await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::MissingActor));
}
