//! The module's private helpers: `card_names` per kind and action,
//! `count_linked_books` (distinct counting, above the bind limit, empty
//! set), `decode_row`'s error reports, and `record_decision`'s stamp.

use omnibus_shared::{CleanupAction, CleanupKind, Decision};

use super::super::*;
use super::{merge_payload, new_pool, seed_authors_with_books, seed_reviewer, seed_suggestion};

// Helpers
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
