//! Series handling on insert: hash-indexed series names collapse into one
//! series with per-book indexes, and an explicit index wins over the one
//! embedded in the name.

use omnibus_shared::{Contributor, EbookMetadata};

use super::super::{sync_books, SyncPlan};
use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::test_support::CoversTempDir;

/// Build an `IndexedBook` carrying only a series name + optional explicit
/// index — the shape a scanned "Name #N" library produces.
fn book_with_series(
    filename: &str,
    title: &str,
    series: &str,
    explicit_index: Option<&str>,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            creators: vec![Contributor {
                name: "Author".into(),
                ..Default::default()
            }],
            series: Some(series.into()),
            series_index: explicit_index.map(Into::into),
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// AC1: a library whose series metadata carries the index inside the name
/// ("Name #1"/"Name #2"/"Name #3") indexes as one series row with three
/// distinctly-indexed books, not three one-book series (#1912).
#[tokio::test]
async fn sync_new_collapses_hash_indexed_series_names_into_one_series_with_per_book_indexes() {
    let _covers = CoversTempDir::new("sync_books_series_hash_index");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                book_with_series("a.epub", "Book 1", "Crowns of Nyaxia #1", None),
                book_with_series("b.epub", "Book 2", "Crowns of Nyaxia #2", None),
                book_with_series("c.epub", "Book 3", "Crowns of Nyaxia #3", None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let series_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series_rows.len(),
        1,
        "the three fragmented names collapse onto one series row: {series_rows:?}"
    );
    assert_eq!(series_rows[0].1, "Crowns of Nyaxia");

    let mut indexes: Vec<f64> = sqlx::query_scalar("SELECT series_index FROM books")
        .fetch_all(&pool)
        .await
        .unwrap();
    indexes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(indexes, vec![1.0, 2.0, 3.0]);
}

/// AC2: an explicit `series_index` always wins over a number embedded in the
/// series name; the embedded index only fills the gap on a book that has no
/// explicit value of its own (#1912).
#[tokio::test]
async fn sync_new_keeps_explicit_series_index_and_only_fills_the_gap_from_the_embedded_name() {
    let _covers = CoversTempDir::new("sync_books_series_explicit_index");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                book_with_series(
                    "explicit.epub",
                    "Explicit",
                    "Gods and Monsters #2",
                    Some("9"),
                ),
                book_with_series("embedded.epub", "Embedded", "Gods and Monsters #3", None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(series_count, 1, "both books share one cleaned series row");

    let explicit_index: f64 =
        sqlx::query_scalar("SELECT series_index FROM books WHERE title = 'Explicit'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        explicit_index, 9.0,
        "explicit index wins over the embedded #2"
    );

    let embedded_index: f64 =
        sqlx::query_scalar("SELECT series_index FROM books WHERE title = 'Embedded'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        embedded_index, 3.0,
        "the embedded index fills the gap when no explicit value was scanned"
    );
}
