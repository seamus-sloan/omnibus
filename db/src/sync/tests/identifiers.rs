//! `book_identifiers` rows across a sync: two ISBNs both persist, a
//! reindex does not duplicate them, and a duplicated identifier collapses
//! to one row.

use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use super::super::*;
use crate::books::{get_book, list_books};
use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::test_support::CoversTempDir;

/// Build a one-book index entry whose OPF carries the given identifiers.
fn book_with_identifiers(filename: &str, idents: Vec<Identifier>) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some("Two ISBNs".into()),
            creators: vec![Contributor {
                name: "Author".into(),
                ..Default::default()
            }],
            identifiers: idents,
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// Count `book_identifiers` rows for one scheme on a given book id.
async fn isbn_row_count(pool: &sqlx::SqlitePool, book_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_identifiers WHERE book_id = ? AND scheme = 'ISBN'",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// F7: a book whose OPF lists a print ISBN-10 and an ebook ISBN-13 under the
/// same `ISBN` scheme must keep BOTH `book_identifiers` rows. Under the old
/// `(book_id, scheme)` PK + `INSERT OR REPLACE`, the second value clobbered
/// the first; the wider `(book_id, scheme, value)` PK + `INSERT OR IGNORE`
/// keeps both, and the read projection surfaces both.
#[tokio::test]
async fn book_with_two_isbns_keeps_both_identifier_rows() {
    let _covers = CoversTempDir::new("two_isbns");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![book_with_identifiers(
            "two.epub",
            vec![
                Identifier {
                    value: "0000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let id = books[0].id;

    // Both rows survive at the table level.
    assert_eq!(
        isbn_row_count(&pool, id).await,
        2,
        "both distinct ISBN values must persist in book_identifiers"
    );

    // And both surface through the read projection (list_books + get_book).
    let mut list_isbns: Vec<&str> = books[0]
        .identifiers
        .iter()
        .filter(|i| i.scheme.as_deref() == Some("ISBN"))
        .map(|i| i.value.as_str())
        .collect();
    list_isbns.sort_unstable();
    assert_eq!(list_isbns, vec!["0000000000", "9780000000000"]);

    let detail = get_book(&pool, id).await.unwrap().unwrap();
    let mut detail_isbns: Vec<&str> = detail
        .identifiers
        .iter()
        .filter(|i| i.scheme.as_deref() == Some("ISBN"))
        .map(|i| i.value.as_str())
        .collect();
    detail_isbns.sort_unstable();
    assert_eq!(detail_isbns, vec!["0000000000", "9780000000000"]);
}

/// Re-indexing the same book (the `replace_books` Removed-then-New path
/// cascade-deletes and relinks identifiers) keeps exactly the two distinct
/// rows — no accumulation, no further collapse.
#[tokio::test]
async fn reindexing_two_isbns_does_not_duplicate_identifier_rows() {
    let _covers = CoversTempDir::new("two_isbns_reindex");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let make = || {
        vec![book_with_identifiers(
            "two.epub",
            vec![
                Identifier {
                    value: "0000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )]
    };

    replace_books(&pool, "/lib", make()).await.unwrap();
    replace_books(&pool, "/lib", make()).await.unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(
        isbn_row_count(&pool, books[0].id).await,
        2,
        "reindex must not duplicate or collapse the two ISBN rows"
    );
}

/// `INSERT OR IGNORE` idempotence: an OPF that lists the exact same
/// `(scheme, value)` tuple twice collapses to a single row — the duplicate is
/// silently ignored, not a PK-violation error.
#[tokio::test]
async fn book_with_duplicate_identifier_dedups_to_one_row() {
    let _covers = CoversTempDir::new("dup_ident");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![book_with_identifiers(
            "dup.epub",
            vec![
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )],
    )
    .await
    .expect("duplicate identifier must dedup, not error");

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(
        isbn_row_count(&pool, books[0].id).await,
        1,
        "exact-duplicate identifier tuple must collapse to one row"
    );
}
