//! `review_counts` and `review_queue`: the per-kind counts by decision,
//! the hydrated pending queue for one kind with its row cap, cached author
//! photo URLs, malformed payloads reported rather than skipped, the names
//! a merge folds away, the atoms a split writes, and merge groups larger
//! than the SQLite bind limit.

use omnibus_shared::{CleanupAction, CleanupKind, Decision};
use sqlx::SqlitePool;

use super::super::*;
use super::{merge_payload, new_pool, seed_authors_with_books, seed_suggestion};

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

// Counts
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

// Queue
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

// Decide
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
