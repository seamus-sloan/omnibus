//! Coverage for the two staleness passes: [`prune_stale_suggestions`] over
//! each payload shape whose target can disappear, and [`prune_undetected`]
//! over a kind whose detection output has shrunk.

use sqlx::SqlitePool;

use super::*;
use crate::cleanup::{detect_authors, review_counts, CleanupPayload};
use crate::pool::init_db;

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

async fn insert_suggestion(
    pool: &SqlitePool,
    kind: CleanupKind,
    action: CleanupAction,
    payload: &CleanupPayload,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO dedup_suggestions (kind, action, payload_json)
         VALUES (?, ?, ?) RETURNING id",
    )
    .bind(kind.as_str())
    .bind(action.as_str())
    .bind(serde_json::to_string(payload).unwrap())
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn pending_count(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM dedup_suggestions WHERE decision = 'pending'")
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_author(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO authors (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_book(pool: &SqlitePool, uuid: &str, title: &str) -> i64 {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(format!("/lib/{uuid}"))
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

fn merge_payload(source_ids: Vec<i64>, canonical_id: i64) -> CleanupPayload {
    CleanupPayload::Merge {
        source_ids,
        source_names: vec!["Gone".to_string()],
        canonical_id,
        canonical_name: "Kept".to_string(),
    }
}

fn rename_payload(uuid: &str, current: &str) -> CleanupPayload {
    CleanupPayload::Rename {
        book_id: 1,
        book_uuid: uuid.to_string(),
        current_title: current.to_string(),
        proposed_title: "Clean".to_string(),
    }
}

// ---------------------------------------------------------------------------
// prune_stale_suggestions
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_stale_suggestions_retires_a_merge_whose_source_author_is_gone() {
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    let gone = insert_author(&pool, "Gone").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(vec![gone], kept),
    )
    .await;
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
    assert_eq!(pending_count(&pool).await, 0);
}

#[tokio::test]
async fn prune_stale_suggestions_retires_a_merge_whose_canonical_author_is_gone() {
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    let gone = insert_author(&pool, "Gone").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(vec![gone], kept),
    )
    .await;
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(kept)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn prune_stale_suggestions_keeps_a_merge_whose_entities_all_still_exist() {
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    let gone = insert_author(&pool, "Gone").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(vec![gone], kept),
    )
    .await;

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_keeps_a_group_merge_that_lost_only_one_source() {
    // A three-way group that loses one duplicate is still a real merge of the
    // two that remain, and the apply primitive tolerates a source id with no
    // rows behind it.
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    let a = insert_author(&pool, "Dup A").await;
    let b = insert_author(&pool, "Dup B").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(vec![a, b], kept),
    )
    .await;
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(a)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_leaves_a_payload_it_cannot_read() {
    // An undecodable row is reported by the queue, not silently destroyed by
    // a prune that could not tell what it named.
    let pool = new_pool().await;
    sqlx::query(
        "INSERT INTO dedup_suggestions (kind, action, payload_json)
         VALUES ('author', 'merge', '{\"type\":\"merge\"}')",
    )
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_leaves_a_merge_payload_missing_its_source_ids() {
    // A payload with a canonical but no `source_ids` array reads as "no
    // surviving source" to `json_each`, which would delete it as stale — the
    // silent destruction of an unreadable row this module promises not to do.
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    sqlx::query(
        "INSERT INTO dedup_suggestions (kind, action, payload_json)
         VALUES ('author', 'merge', json_object('type', 'merge', 'canonical_id', ?))",
    )
    .bind(kept)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_leaves_a_merge_whose_source_ids_is_not_an_array() {
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    sqlx::query(
        "INSERT INTO dedup_suggestions (kind, action, payload_json)
         VALUES ('author', 'merge',
                 json_object('type', 'merge', 'canonical_id', ?, 'source_ids', 'nonsense'))",
    )
    .bind(kept)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_retires_a_delete_for_an_author_already_deleted() {
    let pool = new_pool().await;
    let junk = insert_author(&pool, "calibre (0.7.23)").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Delete,
        &CleanupPayload::Delete {
            entity_id: junk,
            name: "calibre (0.7.23)".to_string(),
        },
    )
    .await;
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(junk)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn prune_stale_suggestions_retires_a_split_for_a_tag_that_no_longer_exists() {
    let pool = new_pool().await;
    let tag: i64 = sqlx::query_scalar("INSERT INTO tags (name) VALUES ('a;b') RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();
    insert_suggestion(
        &pool,
        CleanupKind::Tag,
        CleanupAction::Split,
        &CleanupPayload::Split {
            source_id: tag,
            source_name: "a;b".to_string(),
            atoms: vec!["a".to_string(), "b".to_string()],
            delimiter: ";".to_string(),
        },
    )
    .await;
    sqlx::query("DELETE FROM tags WHERE id = ?")
        .bind(tag)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn prune_stale_suggestions_retires_a_rename_for_a_book_with_a_title_override() {
    let pool = new_pool().await;
    let uuid = "11111111-1111-4111-8111-111111111111";
    insert_book(&pool, uuid, "Maas, Sarah J - A Court").await;
    insert_suggestion(
        &pool,
        CleanupKind::BookTitle,
        CleanupAction::Rename,
        &rename_payload(uuid, "Maas, Sarah J - A Court"),
    )
    .await;
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides)
         VALUES (?, json('{\"title\":\"A Court\"}'))",
    )
    .bind(uuid)
    .execute(&pool)
    .await
    .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn prune_stale_suggestions_retires_a_rename_whose_scanned_title_has_changed() {
    let pool = new_pool().await;
    let uuid = "22222222-2222-4222-8222-222222222222";
    insert_book(&pool, uuid, "Maas, Sarah J - A Court").await;
    insert_suggestion(
        &pool,
        CleanupKind::BookTitle,
        CleanupAction::Rename,
        &rename_payload(uuid, "Maas, Sarah J - A Court"),
    )
    .await;
    sqlx::query("UPDATE books SET title = 'Something Else' WHERE uuid = ?")
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 1);
}

#[tokio::test]
async fn prune_stale_suggestions_keeps_a_rename_whose_book_is_untouched() {
    let pool = new_pool().await;
    let uuid = "33333333-3333-4333-8333-333333333333";
    insert_book(&pool, uuid, "Maas, Sarah J - A Court").await;
    insert_suggestion(
        &pool,
        CleanupKind::BookTitle,
        CleanupAction::Rename,
        &rename_payload(uuid, "Maas, Sarah J - A Court"),
    )
    .await;

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_stale_suggestions_leaves_decided_rows_alone() {
    let pool = new_pool().await;
    let gone = insert_author(&pool, "Gone").await;
    let id = insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Delete,
        &CleanupPayload::Delete {
            entity_id: gone,
            name: "Gone".to_string(),
        },
    )
    .await;
    sqlx::query("UPDATE dedup_suggestions SET decision = 'accepted' WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    let decision: String =
        sqlx::query_scalar("SELECT decision FROM dedup_suggestions WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(decision, "accepted", "the review ledger must survive");
}

#[tokio::test]
async fn prune_stale_suggestions_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    assert!(prune_stale_suggestions(&pool).await.is_err());
}

#[tokio::test]
async fn review_counts_stops_counting_a_suggestion_whose_author_is_gone() {
    let pool = new_pool().await;
    let gone = insert_author(&pool, "calibre (0.7.23)").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Delete,
        &CleanupPayload::Delete {
            entity_id: gone,
            name: "calibre (0.7.23)".to_string(),
        },
    )
    .await;
    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(gone)
        .execute(&pool)
        .await
        .unwrap();

    let counts = review_counts(&pool).await.unwrap();
    let pending = counts
        .iter()
        .find(|(k, _)| *k == CleanupKind::Author)
        .map(|(_, c)| c.pending)
        .unwrap();
    assert_eq!(pending, 0, "the dashboard must not promise a dead card");
}

// ---------------------------------------------------------------------------
// prune_undetected
// ---------------------------------------------------------------------------

#[tokio::test]
async fn prune_undetected_retires_a_pending_row_the_pass_no_longer_emits() {
    let pool = new_pool().await;
    let kept = insert_author(&pool, "Kept").await;
    let gone = insert_author(&pool, "Gone").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(vec![gone], kept),
    )
    .await;

    // Both rows still exist, so the existence prune keeps the suggestion —
    // only the detector knows it is no longer real.
    assert_eq!(prune_stale_suggestions(&pool).await.unwrap(), 0);
    assert_eq!(
        prune_undetected(&pool, &[CleanupKind::Author], &[])
            .await
            .unwrap(),
        1
    );
    assert_eq!(pending_count(&pool).await, 0);
}

#[tokio::test]
async fn prune_undetected_keeps_a_row_the_pass_re_emitted() {
    let pool = new_pool().await;
    let junk = insert_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]").await;
    let book = insert_book(&pool, "44444444-4444-4444-8444-444444444444", "A Book").await;
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book)
        .bind(junk)
        .execute(&pool)
        .await
        .unwrap();
    let fresh = detect_authors(&pool).await.unwrap();
    assert_eq!(fresh.len(), 1, "the junk author is detected");
    insert_suggestion(&pool, fresh[0].kind, fresh[0].action, &fresh[0].payload).await;

    assert_eq!(
        prune_undetected(&pool, &[CleanupKind::Author], &fresh)
            .await
            .unwrap(),
        0
    );
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_undetected_leaves_kinds_the_pass_did_not_cover_alone() {
    let pool = new_pool().await;
    let gone = insert_author(&pool, "Gone").await;
    insert_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Delete,
        &CleanupPayload::Delete {
            entity_id: gone,
            name: "Gone".to_string(),
        },
    )
    .await;

    assert_eq!(
        prune_undetected(&pool, &[CleanupKind::Tag], &[])
            .await
            .unwrap(),
        0
    );
    assert_eq!(pending_count(&pool).await, 1);
}

#[tokio::test]
async fn prune_undetected_propagates_db_error_when_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    assert!(prune_undetected(&pool, &[CleanupKind::Author], &[])
        .await
        .is_err());
}
