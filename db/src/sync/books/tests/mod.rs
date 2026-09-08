//! Direct unit tests for the `sync/books` write-path helpers, split by
//! bucket into the sibling modules below; the shared scan-root and book
//! seeding fixtures live here. Each transactional helper is exercised in
//! isolation against an in-memory DB — the integration-style `replace_books`
//! tests in `sync/tests` cover only the composed happy path.

mod changed;
mod entity_aliases;
mod moved;
mod moved_attachments;
mod new;
mod removed;
mod series;

use omnibus_shared::{Contributor, EbookMetadata, Identifier};
use sqlx::SqlitePool;

use super::shared::{insert_book_row, insert_metadata_links};
use super::EntityAliasMaps;
use crate::ebook::IndexedBook;

/// Insert a `scan_roots` row for `/lib` and return its id — the
/// `library_id` every bucket helper needs.
async fn seed_scan_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Build a fully-populated `IndexedBook` so a single seed touches every
/// per-book link table: authors, tags, series, publisher, language, and
/// identifiers. Used by the link-wipe test to prove all 7 tables clear.
fn book_with_all_links(filename: &str, title: &str) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            publisher: Some("Tor".into()),
            language: Some("en".into()),
            creators: vec![Contributor {
                name: "Ada Lovelace".into(),
                ..Default::default()
            }],
            subjects: vec!["fiction".into()],
            series: Some("Saga".into()),
            series_index: Some("1".into()),
            identifiers: vec![Identifier {
                value: "9780000000000".into(),
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

/// Seed one native book (canonical `books` row + its `book_files` row +
/// all per-book link rows) through the real write helpers, returning its
/// `books.id`. Local to the test module so production code stays lean.
async fn seed_book_with_file(pool: &SqlitePool, library_id: i64, filename: &str) -> i64 {
    let b = book_with_all_links(filename, "Seeded");
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    insert_metadata_links(
        &mut tx,
        inserted.book_id,
        &b.metadata,
        &EntityAliasMaps::default(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    inserted.book_id
}

/// COUNT `book_files` rows for one `book_id`.
async fn book_files_count(pool: &SqlitePool, book_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}
