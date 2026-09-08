//! Acceptance tests for the `books_fts` choke-point, split by sub-topic into
//! the sibling modules below; the invariant assertion, row-snapshot and
//! fixture helpers they share live here.

mod invariant;
mod migration_0078;
mod rebuild;

use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use crate::ebook::IndexedBook;
use crate::test_support::count_rows;

/// Build an `IndexedBook` with a single ISBN identifier so attach/union
/// paths have an ISBN to carry into the target's FTS row.
fn indexed_with_isbn(filename: &str, title: &str, author: &str, isbn: &str) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            creators: vec![Contributor {
                name: author.into(),
                ..Default::default()
            }],
            identifiers: vec![Identifier {
                value: isbn.into(),
                scheme: Some("ISBN".into()),
            }],
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// Assert the standalone FTS twin is in lock-step with `books`: every
/// `books` row has exactly one `books_fts` row, and no `books_fts` row is
/// orphaned (no backing `books` row).
async fn assert_fts_invariant(pool: &sqlx::SqlitePool) {
    // Every `books` row — file-backed or fileless (F2) — keeps exactly one
    // `books_fts` row, so a fileless book stays searchable; the grid/facets
    // hide it via their own `EXISTS book_files` filter.
    let books = count_rows(pool, "SELECT COUNT(*) FROM books").await;
    let fts = count_rows(pool, "SELECT COUNT(*) FROM books_fts").await;
    assert_eq!(books, fts, "books_fts row count must equal books count");

    let missing = count_rows(
        pool,
        "SELECT COUNT(*) FROM books b
          WHERE NOT EXISTS (SELECT 1 FROM books_fts f WHERE f.rowid = b.id)",
    )
    .await;
    assert_eq!(missing, 0, "every book must have a books_fts row");

    let orphans = count_rows(
        pool,
        "SELECT COUNT(*) FROM books_fts f
          WHERE NOT EXISTS (SELECT 1 FROM books b WHERE b.id = f.rowid)",
    )
    .await;
    assert_eq!(orphans, 0, "no books_fts row may point at a deleted book");
}

/// Count `books_fts` rows whose `isbn` column MATCHes the term.
async fn fts_isbn_hits(pool: &sqlx::SqlitePool, isbn: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM books_fts WHERE isbn MATCH ?")
        .bind(isbn)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Count `books_fts` rows whose `genres` column MATCHes the term.
async fn fts_genre_hits(pool: &sqlx::SqlitePool, genre: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM books_fts WHERE genres MATCH ?")
        .bind(genre)
        .fetch_one(pool)
        .await
        .unwrap()
}
