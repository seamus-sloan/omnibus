//! Tests for the one-off backfills: the word-count reset's predicate (read
//! from migration `0084` itself, so the two cannot drift) and the passes that
//! refill what it nulls.

use super::*;
use crate::pool::init_db;
use crate::test_support::seed_minimal_books;

/// Migration `0084`'s reset, read from the migration itself rather than
/// retyped: the two must not be able to drift, and the migration runs
/// against an empty schema here so nothing else exercises its predicate.
const WORD_COUNT_RESET: &str = include_str!("../../../migrations/0084_word_count_reset.sql");

/// Point `ebook_library_path` at `path`. The reset is scoped to the
/// configured root, so without this nothing is in its work set.
async fn configure_library(pool: &SqlitePool, path: &str) {
    sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES ('ebook_library_path', ?)")
        .bind(path)
        .execute(pool)
        .await
        .unwrap();
}

/// Give `book_id` a `book_files` row in `format`, replacing whatever
/// `seed_minimal_books` gave it.
async fn set_only_file_format(pool: &SqlitePool, book_id: i64, format: &str) {
    sqlx::query("DELETE FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, ?, 'f', 1, 1)",
    )
    .bind(book_id)
    .bind(format)
    .execute(pool)
    .await
    .unwrap();
}

async fn word_count_of(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn word_count_reset_nulls_only_rows_the_backfill_can_re_derive() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;
    configure_library(&pool, "/lib").await;
    sqlx::query("UPDATE books SET word_count = 100")
        .execute(&pool)
        .await
        .unwrap();
    // 1 keeps its EPUB file. 2 is ghosted — the file was removed, so the
    // `books` row survives with no `book_files` and nothing to re-read. 3
    // is a comic, which never had a word count derived from a spine.
    sqlx::query("DELETE FROM book_files WHERE book_id = 2")
        .execute(&pool)
        .await
        .unwrap();
    set_only_file_format(&pool, 3, "CBZ").await;

    sqlx::raw_sql(WORD_COUNT_RESET)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        word_count_of(&pool, 1).await,
        None,
        "re-derivable, so reset"
    );
    assert_eq!(
        word_count_of(&pool, 2).await,
        Some(100),
        "a ghost has no EPUB to re-read; nulling would destroy the value"
    );
    assert_eq!(word_count_of(&pool, 3).await, Some(100), "not EPUB-backed");
}

#[tokio::test]
async fn word_count_reset_hands_exactly_its_rows_to_the_backfill() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 2).await;
    configure_library(&pool, "/lib").await;
    sqlx::query("UPDATE books SET word_count = 100")
        .execute(&pool)
        .await
        .unwrap();
    set_only_file_format(&pool, 2, "CBZ").await;

    sqlx::raw_sql(WORD_COUNT_RESET)
        .execute(&pool)
        .await
        .unwrap();
    let candidates = fetch_word_count_candidates(&pool, "/lib").await.unwrap();

    // The reset's guards mirror the backfill's joins, so every row it nulls
    // comes straight back as work — a row it nulled but the backfill
    // skipped would stay NULL forever.
    let ids: Vec<i64> = candidates.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![1]);
}

#[tokio::test]
async fn word_count_reset_leaves_a_root_that_is_no_longer_the_configured_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;
    // A second root that still owns a book after the library was repointed
    // away from it. `settings::prune_orphan_libraries` keeps such a root on
    // purpose, and `backfill_word_counts` is only ever posted with the
    // configured path — so nulling this row would destroy the value with
    // nothing left able to refill it.
    let other: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/old', 'old') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, word_count)
         VALUES ('uuid-old', 'old.epub', ?, '/old/b.epub', 'Old', 'Old', 100)",
    )
    .bind(other)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         SELECT id, 'EPUB', 'old', 1, 1 FROM books WHERE uuid = 'uuid-old'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("UPDATE books SET word_count = 100 WHERE word_count IS NULL")
        .execute(&pool)
        .await
        .unwrap();
    configure_library(&pool, "/lib").await;

    sqlx::raw_sql(WORD_COUNT_RESET)
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        word_count_of(&pool, 1).await,
        None,
        "configured root, reset"
    );
    let stranded: Option<i64> =
        sqlx::query_scalar("SELECT word_count FROM books WHERE uuid = 'uuid-old'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stranded,
        Some(100),
        "the backfill is never posted for this root, so nulling is destructive"
    );
}

/// Empty `updates` must not build any SQL at all — `chunks()` over an
/// empty slice yields zero chunks, so this is really asserting the
/// no-op holds rather than that some degenerate `CASE`/`IN ()` executes.
#[tokio::test]
async fn batch_update_books_column_is_a_noop_for_empty_updates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;

    let mut tx = pool.begin().await.unwrap();
    batch_update_books_column(&mut tx, BooksColumn::WordCount, &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let word_count: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(word_count, None);
}

/// A single-row update must produce valid `CASE id WHEN ? THEN ? END`
/// and `IN (?)` clauses, not just the multi-row shape.
#[tokio::test]
async fn batch_update_books_column_writes_a_single_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 1).await;

    let mut tx = pool.begin().await.unwrap();
    batch_update_books_column(&mut tx, BooksColumn::WordCount, &[(1, 42)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let word_count: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(word_count, Some(42));
}

/// A multi-row update must resolve each id to its own value via the
/// `CASE` branches, not clobber every matched row with one value.
#[tokio::test]
async fn batch_update_books_column_writes_distinct_values_per_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, 3).await;

    let mut tx = pool.begin().await.unwrap();
    batch_update_books_column(
        &mut tx,
        BooksColumn::PageCount,
        &[(1, 10), (2, 20), (3, 30)],
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    for (id, expected) in [(1, 10), (2, 20), (3, 30)] {
        let page_count: Option<i64> =
            sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(page_count, Some(expected));
    }
}
