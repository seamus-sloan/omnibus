//! Tests for the full-library [`library_facets`] aggregate.

use omnibus_shared::FacetCount;

use super::*;
use crate::pool::init_db;
use crate::test_support::seed_discovery_fixture;

fn pairs(facet: &[FacetCount]) -> Vec<(String, i64)> {
    facet.iter().map(|f| (f.value.clone(), f.count)).collect()
}

#[tokio::test]
async fn library_facets_counts_each_facet_group() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let facets = library_facets(&pool, &["/lib"]).await.unwrap();

    assert_eq!(
        pairs(&facets.authors),
        vec![
            ("Ada Lovelace".into(), 3),
            ("Grace Hopper".into(), 1),
            ("Niklaus Wirth".into(), 1),
        ]
    );
    assert_eq!(
        pairs(&facets.series),
        vec![("Saga".into(), 2), ("Pioneers".into(), 1)]
    );
    assert_eq!(pairs(&facets.formats), vec![("epub".into(), 4)]);
    assert_eq!(
        pairs(&facets.tags),
        vec![
            ("fiction".into(), 2),
            ("classic".into(), 1),
            ("essay".into(), 1),
            ("nonfiction".into(), 1),
        ]
    );
}

#[tokio::test]
async fn library_facets_orders_by_count_desc_then_value_asc() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let facets = library_facets(&pool, &["/lib"]).await.unwrap();

    // Highest count first; equal counts break alphabetically.
    assert_eq!(
        facets.authors.first().map(|f| f.value.as_str()),
        Some("Ada Lovelace")
    );
    let tail: Vec<&str> = facets.authors[1..]
        .iter()
        .map(|f| f.value.as_str())
        .collect();
    assert_eq!(tail, vec!["Grace Hopper", "Niklaus Wirth"]);
}

#[tokio::test]
async fn library_facets_excludes_fileless_books() {
    let (pool, _guard) = seed_discovery_fixture().await;
    // A fileless book: a book row with an author link but no book_files.
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES ('fileless', ?, '/lib', 'Fileless Book')
         RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let author_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Fadeaway', 'Fadeaway') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book_id)
        .bind(author_id)
        .execute(&pool)
        .await
        .unwrap();

    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert!(
        facets.authors.iter().all(|f| f.value != "Fadeaway"),
        "a book with no backing file must not contribute to facet counts"
    );
}

#[tokio::test]
async fn library_facets_returns_empty_for_no_paths() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let facets = library_facets(&pool, &[]).await.unwrap();
    assert!(facets.authors.is_empty());
    assert!(facets.series.is_empty());
    assert!(facets.formats.is_empty());
    assert!(facets.tags.is_empty());
}
