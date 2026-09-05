//! Every branch of `decide_suggestion`: accept applies the primitive and
//! stamps the row, reject stamps without applying, the reviewer's edited
//! wording (trimmed, refused when empty or for a kind that cannot carry
//! one), and the pending, not-found, already-reviewed, unsupported,
//! apply-failure and unrecognized-token errors.

use omnibus_shared::{CleanupAction, CleanupKind, Decision};
use sqlx::SqlitePool;

use super::super::*;
use super::{merge_payload, new_pool, seed_authors_with_books, seed_reviewer, seed_suggestion};

async fn stored_decision(pool: &SqlitePool, id: i64) -> String {
    sqlx::query_scalar("SELECT decision FROM dedup_suggestions WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn decide_suggestion_applies_the_merge_and_stamps_an_accepted_row() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    let admin_id = seed_reviewer(&pool).await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;

    decide_suggestion(&pool, id, Decision::Accepted, admin_id, None)
        .await
        .unwrap();

    assert_eq!(stored_decision(&pool, id).await, "accepted");
    // The merge really ran: the source author is gone.
    let survived: Option<i64> = sqlx::query_scalar("SELECT id FROM authors WHERE id = ?")
        .bind(authors[0])
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(survived, None);
}

#[tokio::test]
async fn decide_suggestion_stamps_a_rejected_row_without_applying_anything() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    let admin_id = seed_reviewer(&pool).await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;

    decide_suggestion(&pool, id, Decision::Rejected, admin_id, None)
        .await
        .unwrap();

    assert_eq!(stored_decision(&pool, id).await, "rejected");
    let (decided_by, decided_at): (Option<i64>, Option<i64>) =
        sqlx::query_as("SELECT decided_by, decided_at FROM dedup_suggestions WHERE id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(decided_by, Some(admin_id));
    assert!(decided_at.is_some());
    // Both authors are still there — a rejection applies nothing.
    let remaining: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(remaining, 2);
}

/// Seed a book plus a pending rename suggestion naming it, returning
/// `(uuid, suggestion_id)`.
async fn seed_rename(pool: &SqlitePool) -> (String, i64) {
    let uuid = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa".to_string();
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/rename-lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title)
         VALUES (?, 'f.epub', ?, '/rename-lib/f.epub', 'Shelley, Mary - Frankenstein')
         RETURNING id",
    )
    .bind(&uuid)
    .bind(lib_id)
    .fetch_one(pool)
    .await
    .unwrap();
    let payload = serde_json::json!({
        "type": "rename",
        "book_id": book_id,
        "book_uuid": uuid,
        "current_title": "Shelley, Mary - Frankenstein",
        "proposed_title": "Frankenstein",
    })
    .to_string();
    let id = seed_suggestion(
        pool,
        CleanupKind::BookTitle,
        CleanupAction::Rename,
        &payload,
        Decision::Pending,
    )
    .await;
    (uuid, id)
}

/// The title override a book ended up carrying, if any.
async fn stored_title_override(pool: &SqlitePool, uuid: &str) -> Option<String> {
    sqlx::query_scalar(
        "SELECT json_extract(overrides, '$.title') FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await
    .unwrap()
    .flatten()
}

#[tokio::test]
async fn decide_suggestion_writes_the_reviewers_wording_over_the_proposal() {
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    let (uuid, id) = seed_rename(&pool).await;

    decide_suggestion(
        &pool,
        id,
        Decision::Accepted,
        admin_id,
        Some("Frankenstein; or, The Modern Prometheus"),
    )
    .await
    .unwrap();

    assert_eq!(
        stored_title_override(&pool, &uuid).await.as_deref(),
        Some("Frankenstein; or, The Modern Prometheus")
    );
}

#[tokio::test]
async fn decide_suggestion_writes_the_detected_proposal_when_nothing_was_edited() {
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    let (uuid, id) = seed_rename(&pool).await;

    decide_suggestion(&pool, id, Decision::Accepted, admin_id, None)
        .await
        .unwrap();

    assert_eq!(
        stored_title_override(&pool, &uuid).await.as_deref(),
        Some("Frankenstein")
    );
}

#[tokio::test]
async fn decide_suggestion_trims_the_reviewers_wording() {
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    let (uuid, id) = seed_rename(&pool).await;

    decide_suggestion(
        &pool,
        id,
        Decision::Accepted,
        admin_id,
        Some("  Frankenstein  "),
    )
    .await
    .unwrap();

    assert_eq!(
        stored_title_override(&pool, &uuid).await.as_deref(),
        Some("Frankenstein")
    );
}

#[tokio::test]
async fn decide_suggestion_refuses_an_empty_edited_title_without_stamping_the_row() {
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    let (uuid, id) = seed_rename(&pool).await;

    let err = decide_suggestion(&pool, id, Decision::Accepted, admin_id, Some("   "))
        .await
        .unwrap_err();

    assert!(matches!(err, CleanupStoreError::Refused(m) if m.contains("cannot be empty")));
    assert_eq!(stored_decision(&pool, id).await, "pending");
    assert_eq!(stored_title_override(&pool, &uuid).await, None);
}

#[tokio::test]
async fn decide_suggestion_refuses_an_edit_for_a_kind_whose_apply_cannot_carry_one() {
    // Applying the detector's proposal while reporting success on the
    // reviewer's would write a value nobody chose.
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    let admin_id = seed_reviewer(&pool).await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;

    let err = decide_suggestion(
        &pool,
        id,
        Decision::Accepted,
        admin_id,
        Some("Someone Else"),
    )
    .await
    .unwrap_err();

    assert!(matches!(err, CleanupStoreError::Refused(m) if m.contains("cannot be edited")));
    assert_eq!(stored_decision(&pool, id).await, "pending");
    let survived: Option<i64> = sqlx::query_scalar("SELECT id FROM authors WHERE id = ?")
        .bind(authors[0])
        .fetch_optional(&pool)
        .await
        .unwrap();
    assert_eq!(survived, Some(authors[0]), "the merge must not have run");
}

#[tokio::test]
async fn decide_suggestion_ignores_an_edited_value_on_a_reject() {
    // A reject applies nothing, so there is no value for the edit to conflict
    // with — refusing it would block a reviewer who typed then changed course.
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    let (uuid, id) = seed_rename(&pool).await;

    decide_suggestion(&pool, id, Decision::Rejected, admin_id, Some("Anything"))
        .await
        .unwrap();

    assert_eq!(stored_decision(&pool, id).await, "rejected");
    assert_eq!(stored_title_override(&pool, &uuid).await, None);
}

#[tokio::test]
async fn decide_suggestion_refuses_a_pending_decision() {
    let pool = new_pool().await;
    let err = decide_suggestion(&pool, 1, Decision::Pending, 1, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupStoreError::Refused(ref m) if m == "decision must be accepted or rejected"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn decide_suggestion_reports_not_found_for_an_unknown_id() {
    let pool = new_pool().await;
    let err = decide_suggestion(&pool, 4242, Decision::Accepted, 1, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupStoreError::Refused(ref m) if m == "suggestion not found"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn decide_suggestion_refuses_a_row_that_was_already_reviewed() {
    let pool = new_pool().await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[1], 2),
        Decision::Rejected,
    )
    .await;
    let err = decide_suggestion(&pool, id, Decision::Accepted, 1, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupStoreError::Refused(ref m) if m == "suggestion has already been reviewed"),
        "unexpected error: {err}"
    );
}

#[tokio::test]
async fn decide_suggestion_reports_unsupported_for_a_kind_action_pair_with_no_primitive() {
    let pool = new_pool().await;
    // A *series* split has no apply primitive; accepting one must report
    // rather than record an accept that did nothing.
    let id = seed_suggestion(
        &pool,
        CleanupKind::Series,
        CleanupAction::Split,
        &serde_json::json!({
            "type": "split",
            "source_id": 1,
            "source_name": "a;b",
            "atoms": ["a", "b"],
            "delimiter": ";",
        })
        .to_string(),
        Decision::Pending,
    )
    .await;
    let err = decide_suggestion(&pool, id, Decision::Accepted, 1, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupStoreError::Refused(ref m) if m == "this suggestion cannot be applied automatically"),
        "unexpected error: {err}"
    );
    // And the row stays pending, so the card comes back on the next pass.
    assert_eq!(stored_decision(&pool, id).await, "pending");
}

#[tokio::test]
async fn decide_suggestion_propagates_an_apply_failure_without_stamping_the_row() {
    let pool = new_pool().await;
    let admin_id = seed_reviewer(&pool).await;
    // Neither author id exists, so the merge primitive fails.
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[9001], 9002),
        Decision::Pending,
    )
    .await;
    let err = decide_suggestion(&pool, id, Decision::Accepted, admin_id, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupStoreError::Apply(_)));
    assert_eq!(stored_decision(&pool, id).await, "pending");
}

#[tokio::test]
async fn decide_suggestion_reports_an_unrecognized_token_in_a_stored_row() {
    let pool = new_pool().await;
    // `kind` has no CHECK constraint tying it to this build's enum, so a row
    // from a newer schema decodes as an error rather than a panic.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO dedup_suggestions (kind, action, payload_json)
         VALUES ('publisher', 'merge', ?) RETURNING id",
    )
    .bind(merge_payload(&[1], 2))
    .fetch_one(&pool)
    .await
    .unwrap();
    let err = decide_suggestion(&pool, id, Decision::Accepted, 1, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupStoreError::UnknownToken(t) if t == "publisher"));
}
