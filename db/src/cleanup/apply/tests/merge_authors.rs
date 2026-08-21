//! `apply_merge_authors`: link migration, sort backfill, photo priority
//! reconciliation, the three ways a merge request can be refused, alias
//! preservation across a prior merge, and the claim-then-mutate ordering
//! that keeps two concurrent `undo()` calls on the same merge from racing.

use super::super::*;
use super::{
    alias_canonical, author_photo_source, author_position, author_row, count_rows, fts_authors,
    insert_author, insert_author_photo, insert_book, link_author, new_pool, seed_root,
    tally_race_outcomes, undo,
};

#[tokio::test]
async fn apply_merge_authors_moves_link_writes_alias_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Sandy Brandon", None).await;
    let canonical = insert_author(&pool, "Brandon Sandy", Some("Sandy, Brandon")).await;
    link_author(&pool, book, source, 2).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 1);
    assert_eq!(
        author_position(&pool, book, canonical).await,
        Some(2),
        "canonical had no prior link, so it inherits the source's position"
    );
    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        Some(canonical)
    );
    assert!(fts_authors(&pool, book).await.contains("Brandon Sandy"));
    assert!(!fts_authors(&pool, book).await.contains("Sandy Brandon"));

    undo(&pool, log_id).await.unwrap();

    let (restored_id, restored_sort) = author_row(&pool, "Sandy Brandon").await.unwrap();
    assert_eq!(restored_sort, None);
    assert_eq!(author_position(&pool, book, restored_id).await, Some(2));
    assert_eq!(
        author_position(&pool, book, canonical).await,
        None,
        "the canonical's merge-created link must be removed on undo"
    );
    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        None
    );
    assert!(fts_authors(&pool, book).await.contains("Sandy Brandon"));
    assert!(!fts_authors(&pool, book).await.contains("Brandon Sandy"));
}

#[tokio::test]
async fn apply_merge_authors_backfills_null_sort_and_undo_clears_it() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Bar Foo", Some("Foo, Bar")).await;
    let canonical = insert_author(&pool, "Foo Bar", None).await;
    link_author(&pool, book, source, 0).await;
    link_author(&pool, book, canonical, 1).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    let (_, canonical_sort) = author_row(&pool, "Foo Bar").await.unwrap();
    assert_eq!(canonical_sort.as_deref(), Some("Foo, Bar"));
    // The book already had a canonical link before the merge, so the
    // source's own link must NOT survive as a second row for the same book.
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book}")
        )
        .await,
        1
    );

    undo(&pool, log_id).await.unwrap();

    let (_, canonical_sort_after_undo) = author_row(&pool, "Foo Bar").await.unwrap();
    assert_eq!(
        canonical_sort_after_undo, None,
        "undo must restore the backfilled sort to NULL, not just to its own prior value"
    );
    assert_eq!(
        author_position(&pool, book, canonical).await,
        Some(1),
        "the canonical's pre-existing link must survive the round trip untouched"
    );
}

#[tokio::test]
async fn apply_merge_authors_reconciles_photos_by_priority_and_undo_restores_both() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Manual Source", None).await;
    let canonical = insert_author(&pool, "Letter Canonical", None).await;
    link_author(&pool, book, source, 0).await;
    insert_author_photo(&pool, source, "manual", Some(b"manual-bytes")).await;
    insert_author_photo(&pool, canonical, "letter", None).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(
        author_photo_source(&pool, canonical).await.as_deref(),
        Some("manual"),
        "manual outranks letter, so the canonical adopts the source's photo"
    );

    undo(&pool, log_id).await.unwrap();

    assert_eq!(
        author_photo_source(&pool, canonical).await.as_deref(),
        Some("letter"),
        "undo must restore the canonical's own pre-merge photo"
    );
    let (restored_id, _) = author_row(&pool, "Manual Source").await.unwrap();
    assert_eq!(
        author_photo_source(&pool, restored_id).await.as_deref(),
        Some("manual"),
        "the recreated source's photo must follow it"
    );
}

#[tokio::test]
async fn apply_merge_authors_returns_not_found_for_unknown_source() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Canonical", None).await;
    let err = apply_merge_authors(&pool, &[9999], canonical, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::NotFound(9999)));
}

#[tokio::test]
async fn apply_merge_authors_returns_empty_sources_for_an_empty_list() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Canonical", None).await;
    let err = apply_merge_authors(&pool, &[], canonical, None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupApplyError::InvalidRequest(ref m) if m == "merge requires at least one source entity"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn apply_merge_authors_returns_canonical_is_source_when_they_collide() {
    let pool = new_pool().await;
    let a = insert_author(&pool, "A", None).await;
    let err = apply_merge_authors(&pool, &[a], a, None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupApplyError::InvalidRequest(ref m) if m == &format!("merge source and canonical are the same entity: {a}")),
        "unexpected error: {err}"
    );
}

/// A merge overwrites an `entity_aliases` row that already pointed
/// somewhere else (from an earlier, separately-applied merge). Undoing the
/// later merge must restore that earlier mapping, not delete it outright —
/// deleting would make the earlier merge's absorbed name resolve to
/// nothing.
#[tokio::test]
async fn apply_merge_authors_undo_restores_a_preexisting_alias_rather_than_deleting_it() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let earlier_canonical = insert_author(&pool, "Earlier Canonical", None).await;
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id, created_at)
         VALUES ('author', 'Sandy Brandon', ?, 111)",
    )
    .bind(earlier_canonical)
    .execute(&pool)
    .await
    .unwrap();

    let source = insert_author(&pool, "Sandy Brandon", None).await;
    let canonical = insert_author(&pool, "Brandon Sandy", Some("Sandy, Brandon")).await;
    link_author(&pool, book, source, 0).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();
    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        Some(canonical),
        "the merge overwrites the alias to point at its own canonical"
    );

    undo(&pool, log_id).await.unwrap();

    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        Some(earlier_canonical),
        "undo must restore the pre-merge alias mapping, not delete it"
    );
}

/// Two `undo()` calls on the same merge log entry, released at the same
/// instant via a barrier (mirroring
/// `merge_metadata_overrides_concurrent_saves_dont_drop_writes`'s real-
/// contention shape). Before the ordering fix, `mark_undone_tx` ran *after*
/// `undo_merge`'s replay, so the loser could recreate the source author a
/// second time before discovering the entry was already undone. With the
/// claim moved first, the loser's claim affects zero rows and it exits
/// before touching `authors` at all — exactly one caller succeeds, and the
/// end state matches a single clean undo (one restored author row, not two,
/// and no leftover duplicate link).
#[tokio::test]
async fn undo_concurrent_calls_on_a_merge_race_to_exactly_one_success() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Sandy Brandon", None).await;
    let canonical = insert_author(&pool, "Brandon Sandy", Some("Sandy, Brandon")).await;
    link_author(&pool, book, source, 2).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
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

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM authors").await,
        2,
        "exactly the recreated source plus the untouched canonical — a double \
         replay would have recreated the source author a second time"
    );
    let (restored_id, _) = author_row(&pool, "Sandy Brandon").await.unwrap();
    assert_eq!(author_position(&pool, book, restored_id).await, Some(2));
}
