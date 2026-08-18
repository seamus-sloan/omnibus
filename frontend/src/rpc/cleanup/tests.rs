//! Tests for the cleanup review store — the `dedup_suggestions` read/decide
//! queries this module owns, which the thin server-function wrappers around
//! them are covered by transitively.

use omnibus_shared::{CleanupAction, CleanupKind, Decision};
use sqlx::SqlitePool;

use super::{
    card_names, count_linked_books, counts_by_kind, decode_row, pending_queue, record_decision,
    CleanupStoreError, StoredPayload,
};

async fn pool() -> SqlitePool {
    omnibus_db::init_db("sqlite::memory:").await.unwrap()
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

#[tokio::test]
async fn counts_by_kind_reports_every_kind_at_zero_when_nothing_is_detected() {
    let pool = pool().await;
    let counts = counts_by_kind(&pool).await.unwrap();
    assert_eq!(counts.len(), 4);
    assert!(counts
        .iter()
        .all(|(_, c)| c.pending == 0 && c.accepted == 0 && c.rejected == 0));
}

#[tokio::test]
async fn counts_by_kind_buckets_rows_by_kind_and_decision() {
    let pool = pool().await;
    seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[1], 2),
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

    let counts = counts_by_kind(&pool).await.unwrap();
    let by = |k: CleanupKind| counts.iter().find(|(kind, _)| *kind == k).unwrap().1;
    assert_eq!(by(CleanupKind::Author).pending, 1);
    assert_eq!(by(CleanupKind::Author).rejected, 1);
    assert_eq!(by(CleanupKind::Tag).accepted, 1);
    assert_eq!(by(CleanupKind::Series).pending, 0);
}

#[tokio::test]
async fn pending_queue_returns_only_pending_rows_of_the_requested_kind() {
    let pool = pool().await;
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

    let cards = pending_queue(&pool, CleanupKind::Author, 50).await.unwrap();
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
async fn pending_queue_honors_the_row_limit() {
    let pool = pool().await;
    for n in 0..3 {
        seed_suggestion(
            &pool,
            CleanupKind::Series,
            CleanupAction::Merge,
            &merge_payload(&[n], n + 100),
            Decision::Pending,
        )
        .await;
    }
    let cards = pending_queue(&pool, CleanupKind::Series, 2).await.unwrap();
    assert_eq!(cards.len(), 2);
}

#[test]
fn card_names_names_the_single_entity_a_two_way_merge_removes() {
    let payload = StoredPayload::Merge {
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
    let payload = StoredPayload::Merge {
        source_ids: vec![1, 2],
        source_names: vec!["A".into(), "B".into()],
        canonical_id: 3,
        canonical_name: "C".into(),
    };
    assert_eq!(card_names(&payload), ("C".to_string(), None));
}

#[test]
fn card_names_pairs_the_current_and_proposed_title_for_a_rename() {
    let payload = StoredPayload::Rename {
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
    let split = StoredPayload::Split {
        source_id: 1,
        source_name: "sci-fi;fantasy".into(),
        atoms: vec!["sci-fi".into(), "fantasy".into()],
        delimiter: ";".into(),
    };
    assert_eq!(card_names(&split), ("sci-fi;fantasy".to_string(), None));

    let delete = StoredPayload::Delete {
        entity_id: 4,
        name: "Calibre".into(),
    };
    assert_eq!(card_names(&delete), ("Calibre".to_string(), None));
}

#[tokio::test]
async fn count_linked_books_counts_a_book_once_even_when_two_group_members_share_it() {
    let pool = pool().await;
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
async fn count_linked_books_returns_zero_for_an_empty_id_set() {
    let pool = pool().await;
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
    let pool = pool().await;
    let id = seed_suggestion(
        &pool,
        CleanupKind::Author,
        CleanupAction::Merge,
        &merge_payload(&[1], 2),
        Decision::Pending,
    )
    .await;

    // `decided_by` carries a real FK to `users`, so the reviewer has to exist.
    omnibus_db::auth::create_user(&pool, "reviewer", "correct horse battery staple")
        .await
        .unwrap();
    let admin_id = omnibus_db::auth::get_user_by_username(&pool, "reviewer")
        .await
        .unwrap()
        .unwrap()
        .id;

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
