//! `apply_tag_split`: atom creation and undo, the too-few-atoms refusal,
//! batched linking at scale (many books, several atoms), undo touching
//! only the split's own links, and the query count staying flat as the
//! linked-book count grows.

use super::super::*;
use super::{count_rows, fts_tags, insert_book, insert_tag, link_tag, new_pool, seed_root, undo};

#[tokio::test]
async fn apply_tag_split_creates_atoms_and_undo_restores_the_source_tag() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_tag(&pool, "scifi;fantasy").await;
    link_tag(&pool, book, source).await;

    let atoms = vec!["scifi".to_string(), "fantasy".to_string()];
    let log_id = apply_tag_split(&pool, source, ";", &atoms, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
        )
        .await,
        2
    );
    let fts = fts_tags(&pool, book).await;
    assert!(fts.contains("scifi") && fts.contains("fantasy"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
        )
        .await,
        1
    );
    assert!(fts_tags(&pool, book).await.contains("scifi;fantasy"));
}

#[tokio::test]
async fn apply_tag_split_returns_too_few_atoms_for_a_single_atom_list() {
    let pool = new_pool().await;
    let source = insert_tag(&pool, "solo").await;
    let err = apply_tag_split(&pool, source, ";", &["solo".to_string()], None, None)
        .await
        .unwrap_err();
    assert!(
        matches!(err, CleanupApplyError::InvalidRequest(ref m) if m == "tag split requires at least two atoms"),
        "unexpected error: {err}"
    );
}

/// A source tag linked to many books, split into several atoms, must land
/// every (book, atom) pair via the batched `move_links`-per-atom path — the
/// same end state the old per-book-per-atom loop produced, just without the
/// O(atoms × books) round trips.
#[tokio::test]
async fn apply_tag_split_links_every_book_in_a_larger_linked_set() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let source = insert_tag(&pool, "scifi;fantasy;horror").await;
    let mut books = Vec::new();
    for i in 0..25 {
        let book = insert_book(&pool, lib, &format!("book-{i}"), &format!("Title {i}")).await;
        link_tag(&pool, book, source).await;
        books.push(book);
    }

    let atoms = vec![
        "scifi".to_string(),
        "fantasy".to_string(),
        "horror".to_string(),
    ];
    let log_id = apply_tag_split(&pool, source, ";", &atoms, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 3);
    for &book in &books {
        assert_eq!(
            count_rows(
                &pool,
                &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
            )
            .await,
            3,
            "every book must be linked to all three atoms"
        );
    }

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
    for &book in &books {
        assert_eq!(
            count_rows(
                &pool,
                &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
            )
            .await,
            1
        );
    }
}

/// `undo_split` used to resolve and delete each `(book, atom)` pair one at
/// a time, so a real regression would delete every row naming an atom id
/// rather than only the ones the split itself created. A book that links an
/// atom independently — after the split, never part of `snap.links` — must
/// survive the undo untouched.
#[tokio::test]
async fn undo_split_removes_only_the_splits_own_atom_links_leaving_unrelated_links_intact() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book_a = insert_book(&pool, lib, "book-a", "Title A").await;
    let book_b = insert_book(&pool, lib, "book-b", "Title B").await;
    let source = insert_tag(&pool, "scifi;fantasy").await;
    link_tag(&pool, book_a, source).await;
    link_tag(&pool, book_b, source).await;

    let atoms = vec!["scifi".to_string(), "fantasy".to_string()];
    let log_id = apply_tag_split(&pool, source, ";", &atoms, None, None)
        .await
        .unwrap();

    // A third book links "scifi" independently, after the split — outside
    // `snap.links`, so this row must survive the undo below.
    let book_c = insert_book(&pool, lib, "book-c", "Title C").await;
    let scifi_id: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = 'scifi'")
        .fetch_one(&pool)
        .await
        .unwrap();
    link_tag(&pool, book_c, scifi_id).await;

    undo(&pool, log_id).await.unwrap();

    for &book in &[book_a, book_b] {
        assert_eq!(
            count_rows(
                &pool,
                &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
            )
            .await,
            1,
            "book must hold only the recreated source tag after undo"
        );
    }
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book_c}")
        )
        .await,
        1,
        "book_c's independent scifi link must survive the undo"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM tags WHERE name = 'scifi'").await,
        1,
        "scifi itself must survive since book_c still references it"
    );
}

/// Counts `tracing` events sqlx emits (target `"sqlx::query"`, one per
/// executed statement) while installed as the default subscriber. Mirrors
/// the `QueryCounter` pattern in `db/src/epub_rewrite/tests.rs` and
/// `db/src/cross_format/tests/`; every `Subscriber` method besides `event`
/// is a no-op.
struct QueryCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for QueryCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        if event.metadata().target() == "sqlx::query" {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// The query count `undo(log_id)` issues to reverse a tag split whose
/// source was linked to `book_count` books before the split, split into two
/// atoms.
async fn undo_split_query_count(book_count: usize) -> usize {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let source = insert_tag(&pool, "scifi;fantasy").await;
    for i in 0..book_count {
        let book = insert_book(&pool, lib, &format!("book-{i}"), &format!("Title {i}")).await;
        link_tag(&pool, book, source).await;
    }

    let atoms = vec!["scifi".to_string(), "fantasy".to_string()];
    let log_id = apply_tag_split(&pool, source, ";", &atoms, None, None)
        .await
        .unwrap();

    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let guard = tracing::subscriber::set_default(QueryCounter(count.clone()));
    undo(&pool, log_id).await.unwrap();
    drop(guard);
    count.load(std::sync::atomic::Ordering::SeqCst)
}

/// The naive replay this test guards against issued one `lookup_tag_id` +
/// conditional `delete_link` per `(book, atom)` pair, so its query count
/// scaled with `books × atoms`. The batched replacement resolves atoms and
/// applies the link insert/delete as one statement per chunk, so as long as
/// both book counts stay under one chunk (`SPLIT_UNDO_CHUNK` = 450) the
/// query count must be identical regardless of how many books were linked.
#[tokio::test]
async fn undo_split_query_count_does_not_grow_with_linked_book_count() {
    let few = undo_split_query_count(2).await;
    let many = undo_split_query_count(200).await;
    assert_eq!(
        few, many,
        "undo_split's query count must not grow with the number of linked \
         books: {few} queries for 2 books vs {many} for 200"
    );
}
