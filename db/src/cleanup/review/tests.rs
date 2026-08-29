//! Tests for the `dedup_suggestions` review queue: the per-kind counts, the
//! hydrated pending queue, and every branch of [`decide_suggestion`] —
//! including each error variant a caller renders.

use omnibus_shared::{CleanupAction, CleanupKind, Decision};
use sqlx::SqlitePool;

use super::*;
use crate::pool::init_db;

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

/// Insert one `dedup_suggestions` row and return its id.
async fn seed_suggestion(
    pool: &SqlitePool,
    kind: CleanupKind,
    action: CleanupAction,
    payload_json: &str,
    decision: Decision,
) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO dedup_suggestions (kind, action, payload_json, decision)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(kind.as_str())
    .bind(action.as_str())
    .bind(payload_json)
    .bind(decision.as_str())
    .fetch_one(pool)
    .await
    .unwrap()
}

fn merge_payload(source_ids: &[i64], canonical_id: i64) -> String {
    let names: Vec<String> = source_ids.iter().map(|i| format!("Source {i}")).collect();
    serde_json::json!({
        "type": "merge",
        "source_ids": source_ids,
        "source_names": names,
        "canonical_id": canonical_id,
        "canonical_name": "Canonical Name",
    })
    .to_string()
}

/// Seed `count` authors plus one book linked to each, returning the author ids.
async fn seed_authors_with_books(pool: &SqlitePool, count: i64) -> Vec<i64> {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for n in 0..count {
        let author_id: i64 =
            sqlx::query_scalar("INSERT INTO authors (name) VALUES (?) RETURNING id")
                .bind(format!("Author {n}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, scan_key, library_id, path, title)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(format!("uuid-{n}"))
        .bind(format!("book-{n}.epub"))
        .bind(lib_id)
        .bind(format!("/lib/book-{n}.epub"))
        .bind(format!("Book {n}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO books_authors_link (book, author) VALUES (?, ?)")
            .bind(book_id)
            .bind(author_id)
            .execute(pool)
            .await
            .unwrap();
        ids.push(author_id);
    }
    ids
}

/// Seed `count` series plus one book linked to each, returning the series ids.
/// The sibling of [`seed_authors_with_books`] for the kinds whose suggestions
/// must survive the staleness prune the read paths run.
async fn seed_series_with_books(pool: &SqlitePool, count: i64) -> Vec<i64> {
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/series-lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let mut ids = Vec::new();
    for n in 0..count {
        let series_id: i64 =
            sqlx::query_scalar("INSERT INTO series (name) VALUES (?) RETURNING id")
                .bind(format!("Series {n}"))
                .fetch_one(pool)
                .await
                .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, scan_key, library_id, path, title)
             VALUES (?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(format!("series-uuid-{n}"))
        .bind(format!("series-book-{n}.epub"))
        .bind(lib_id)
        .bind(format!("/series-lib/series-book-{n}.epub"))
        .bind(format!("Series Book {n}"))
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?, ?)")
            .bind(book_id)
            .bind(series_id)
            .execute(pool)
            .await
            .unwrap();
        ids.push(series_id);
    }
    ids
}

/// `dedup_suggestions.decided_by` carries a real FK to `users`, so a decide
/// test needs an actual reviewer row.
async fn seed_reviewer(pool: &SqlitePool) -> i64 {
    crate::auth::create_user(pool, "reviewer", "correct horse battery staple")
        .await
        .unwrap();
    crate::auth::get_user_by_username(pool, "reviewer")
        .await
        .unwrap()
        .unwrap()
        .id
}

async fn stored_decision(pool: &SqlitePool, id: i64) -> String {
    sqlx::query_scalar("SELECT decision FROM dedup_suggestions WHERE id = ?")
        .bind(id)
        .fetch_one(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// Counts
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_counts_reports_every_kind_at_zero_when_nothing_is_detected() {
    let pool = new_pool().await;
    let counts = review_counts(&pool).await.unwrap();
    assert_eq!(counts.len(), 4);
    assert!(counts
        .iter()
        .all(|(_, c)| c.pending == 0 && c.accepted == 0 && c.rejected == 0));
}

#[tokio::test]
async fn review_counts_buckets_rows_by_kind_and_decision() {
    let pool = new_pool().await;
    // The pending row names real authors: `review_counts` prunes stale rows
    // first, so a suggestion pointing at ids nothing backs would be retired
    // before it could be counted.
    let authors = seed_authors_with_books(&pool, 2).await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[3], 4),
        Decision::Rejected,
    )
    .await;
    seed_suggestion(
        &pool,
        CleanupKind::Tag,
        CleanupAction::Merge,
        &merge_payload(&[5], 6),
        Decision::Accepted,
    )
    .await;

    let counts = review_counts(&pool).await.unwrap();
    let by = |k: CleanupKind| counts.iter().find(|(kind, _)| *kind == k).unwrap().1;
    assert_eq!(by(CleanupKind::Author).pending, 1);
    assert_eq!(by(CleanupKind::Author).rejected, 1);
    assert_eq!(by(CleanupKind::Tag).accepted, 1);
    assert_eq!(by(CleanupKind::Series).pending, 0);
}

#[tokio::test]
async fn review_counts_propagates_a_db_error_when_the_pool_is_closed() {
    let pool = new_pool().await;
    pool.close().await;
    let err = review_counts(&pool).await.unwrap_err();
    assert!(matches!(err, CleanupStoreError::Db(_)));
}

// ---------------------------------------------------------------------------
// Queue
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_queue_returns_only_pending_rows_of_the_requested_kind() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    let pending = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[1]], authors[0]),
        Decision::Rejected,
    )
    .await;
    seed_suggestion(
        &pool,
        CleanupKind::Tag,
        CleanupAction::Merge,
        &merge_payload(&[9], 10),
        Decision::Pending,
    )
    .await;

    let cards = review_queue(&pool, CleanupKind::Author, 50).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].id, pending);
    assert_eq!(cards[0].primary_name, "Canonical Name");
    assert_eq!(cards[0].decision, Decision::Pending);
    // One book per author, and the two are distinct books.
    assert_eq!(cards[0].book_count, 2);
    // No `author_photos` row was seeded, so the card carries no photo.
    assert_eq!(cards[0].photo_url, None);
}

#[tokio::test]
async fn review_queue_clamps_the_row_limit_to_the_page_maximum() {
    let pool = new_pool().await;
    let series = seed_series_with_books(&pool, 6).await;
    for pair in series.chunks(2) {
        seed_suggestion(
            &pool,
            CleanupKind::Series,
            CleanupAction::Merge,
            &merge_payload(&[pair[0]], pair[1]),
            Decision::Pending,
        )
        .await;
    }
    let page = |limit| review_queue(&pool, CleanupKind::Series, limit);
    assert_eq!(page(2).await.unwrap().len(), 2);
    // A caller asking for everything gets the page cap, not the backlog.
    assert_eq!(page(REVIEW_QUEUE_MAX * 10).await.unwrap().len(), 3);
    // A zero limit still returns a card rather than an empty page.
    assert_eq!(page(0).await.unwrap().len(), 1);
}

#[tokio::test]
async fn review_queue_surfaces_the_author_photo_url_when_bytes_are_cached() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, bytes, mime)
         VALUES (?, 'manual', ?, 'image/jpeg')",
    )
    .bind(authors[1])
    .bind(vec![0u8, 1, 2])
    .execute(&pool)
    .await
    .unwrap();
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0]], authors[1]),
        Decision::Pending,
    )
    .await;

    let cards = review_queue(&pool, CleanupKind::Author, 50).await.unwrap();
    assert_eq!(
        cards[0].photo_url,
        Some(format!("/api/authors/{}/photo", authors[1]))
    );
}

#[tokio::test]
async fn review_queue_reports_a_malformed_payload_rather_than_skipping_the_row() {
    let pool = new_pool().await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        "{\"type\":\"merge\"}",
        Decision::Pending,
    )
    .await;
    let err = review_queue(&pool, CleanupKind::Author, 50)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupStoreError::Payload(_)));
}

// ---------------------------------------------------------------------------
// Decide
// ---------------------------------------------------------------------------

#[tokio::test]
async fn review_queue_names_every_record_a_merge_folds_away() {
    // A three-way group has no single "other" name, so `secondary_name` is
    // None — the card has to carry the whole list or it can only say "some
    // records".
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 3).await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[authors[0], authors[1]], authors[2]),
        Decision::Pending,
    )
    .await;

    let cards = review_queue(&pool, CleanupKind::Author, 50).await.unwrap();
    assert_eq!(cards[0].secondary_name, None);
    assert_eq!(
        cards[0].source_names,
        vec!["Source 1".to_string(), "Source 2".to_string()]
    );
    assert!(cards[0].proposed_parts.is_empty());
}

#[tokio::test]
async fn review_queue_carries_the_atoms_a_split_would_write() {
    // Without them the card shows the scanned tag on both sides, which reads
    // as a change that isn't one.
    let pool = new_pool().await;
    let tag: i64 = sqlx::query_scalar("INSERT INTO tags (name) VALUES ('a;b') RETURNING id")
        .fetch_one(&pool)
        .await
        .unwrap();
    let payload = serde_json::json!({
        "type": "split",
        "source_id": tag,
        "source_name": "a;b",
        "atoms": ["a", "b"],
        "delimiter": ";",
    })
    .to_string();
    seed_suggestion(
        &pool,
        CleanupKind::Tag,
        CleanupAction::Split,
        &payload,
        Decision::Pending,
    )
    .await;

    let cards = review_queue(&pool, CleanupKind::Tag, 50).await.unwrap();
    assert_eq!(
        cards[0].proposed_parts,
        vec!["a".to_string(), "b".to_string()]
    );
    assert!(cards[0].source_names.is_empty());
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

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

#[test]
fn card_names_names_the_single_entity_a_two_way_merge_removes() {
    let payload = CleanupPayload::Merge {
        source_ids: vec![1],
        source_names: vec!["Wollstonecraft, Mary".into()],
        canonical_id: 2,
        canonical_name: "Mary Shelley".into(),
    };
    assert_eq!(
        card_names(&payload),
        (
            "Mary Shelley".to_string(),
            Some("Wollstonecraft, Mary".into())
        )
    );
}

#[test]
fn card_names_omits_the_secondary_name_when_a_merge_group_has_several_sources() {
    let payload = CleanupPayload::Merge {
        source_ids: vec![1, 2],
        source_names: vec!["A".into(), "B".into()],
        canonical_id: 3,
        canonical_name: "C".into(),
    };
    assert_eq!(card_names(&payload), ("C".to_string(), None));
}

#[test]
fn card_names_pairs_the_current_and_proposed_title_for_a_rename() {
    let payload = CleanupPayload::Rename {
        book_id: 7,
        book_uuid: "uuid-1".into(),
        current_title: "Dracula (Annotated Edition)".into(),
        proposed_title: "Dracula".into(),
    };
    assert_eq!(
        card_names(&payload),
        (
            "Dracula (Annotated Edition)".to_string(),
            Some("Dracula".into())
        )
    );
}

#[test]
fn card_names_uses_the_source_name_alone_for_a_split_and_a_delete() {
    let split = CleanupPayload::Split {
        source_id: 1,
        source_name: "sci-fi;fantasy".into(),
        atoms: vec!["sci-fi".into(), "fantasy".into()],
        delimiter: ";".into(),
    };
    assert_eq!(card_names(&split), ("sci-fi;fantasy".to_string(), None));

    let delete = CleanupPayload::Delete {
        entity_id: 4,
        name: "Calibre".into(),
    };
    assert_eq!(card_names(&delete), ("Calibre".to_string(), None));
}

#[tokio::test]
async fn count_linked_books_counts_a_book_once_even_when_two_group_members_share_it() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    // Link the first book to the second author too — the shared book must not
    // be double-counted across the merge group.
    sqlx::query(
        "INSERT INTO books_authors_link (book, author)
         SELECT book, ? FROM books_authors_link WHERE author = ?",
    )
    .bind(authors[1])
    .bind(authors[0])
    .execute(&pool)
    .await
    .unwrap();

    let count = count_linked_books(&pool, "books_authors_link", "author", &authors)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn count_linked_books_counts_a_group_larger_than_the_sqlite_bind_limit() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    // Past SQLITE_MAX_VARIABLE_NUMBER on the SQLite this links (32766), so a
    // generated `IN (?, ?, …)` list fails to bind outright. Nothing caps a
    // merge group's size, so the id set has to survive being arbitrarily large.
    let mut ids: Vec<i64> = (100_000..133_000).collect();
    ids.extend(&authors);
    assert!(ids.len() > 32_766);

    let count = count_linked_books(&pool, "books_authors_link", "author", &ids)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn review_queue_hydrates_a_merge_group_larger_than_the_sqlite_bind_limit() {
    let pool = new_pool().await;
    let authors = seed_authors_with_books(&pool, 2).await;
    let mut source_ids: Vec<i64> = (100_000..133_000).collect();
    source_ids.push(authors[0]);
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&source_ids, authors[1]),
        Decision::Pending,
    )
    .await;

    // The whole page must still hydrate — the symptom of a bind overflow is a
    // review queue that errors out rather than one card that reads wrong.
    let cards = review_queue(&pool, CleanupKind::Author, 50).await.unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].book_count, 2);
}

#[tokio::test]
async fn count_linked_books_returns_zero_for_an_empty_id_set() {
    let pool = new_pool().await;
    let count = count_linked_books(&pool, "books_authors_link", "author", &[])
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn decode_row_reports_an_unrecognized_kind_token() {
    let err = decode_row((
        1,
        "publisher".into(),
        "merge".into(),
        "pending".into(),
        merge_payload(&[1], 2),
        0,
    ))
    .unwrap_err();
    assert!(matches!(err, CleanupStoreError::UnknownToken(t) if t == "publisher"));
}

#[test]
fn decode_row_reports_a_malformed_payload() {
    let err = decode_row((
        1,
        "author".into(),
        "merge".into(),
        "pending".into(),
        "{\"type\":\"merge\"}".into(),
        0,
    ))
    .unwrap_err();
    assert!(matches!(err, CleanupStoreError::Payload(_)));
}

#[tokio::test]
async fn record_decision_stamps_the_decision_and_the_reviewing_admin() {
    let pool = new_pool().await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[1], 2),
        Decision::Pending,
    )
    .await;
    let admin_id = seed_reviewer(&pool).await;

    record_decision(&pool, id, Decision::Rejected, admin_id)
        .await
        .unwrap();

    let (decision, decided_by, decided_at): (String, Option<i64>, Option<i64>) = sqlx::query_as(
        "SELECT decision, decided_by, decided_at FROM dedup_suggestions WHERE id = ?",
    )
    .bind(id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(decision, "rejected");
    assert_eq!(decided_by, Some(admin_id));
    assert!(decided_at.is_some());
}
