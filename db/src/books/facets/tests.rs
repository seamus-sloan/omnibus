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

#[tokio::test]
async fn author_facets_truncates_at_facet_limit_and_stays_ordered_when_distinct_count_exceeds_cap()
{
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = FACET_LIMIT + 25;
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // One book per distinct author, so the group count ties at 1 for every
    // author and the `ORDER BY count DESC, value ASC` cap keeps the
    // lexicographically-first FACET_LIMIT names.
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1
            UNION ALL
            SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, scan_key, library_id, path, title)
        SELECT 'uuid-' || i, 'b' || i || '.epub', ?, '/lib/b' || i, 'Title ' || i
          FROM n
        ",
    )
    .bind(total)
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         SELECT id, 'EPUB', 'b' || id, 1, 1 FROM books WHERE library_id = ?",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    // Author name derived straight from the book's own id, so it's paired
    // with exactly one book regardless of the ids sqlite actually assigned.
    sqlx::query(
        "INSERT INTO authors (name, sort)
         SELECT 'Author ' || printf('%06d', id), 'Author ' || printf('%06d', id)
           FROM books WHERE library_id = ?",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books_authors_link (book, author, position)
         SELECT b.id, a.id, 0
           FROM books b
           JOIN authors a ON a.name = 'Author ' || printf('%06d', b.id)
          WHERE b.library_id = ?",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();

    let min_id: i64 = sqlx::query_scalar("SELECT MIN(id) FROM books WHERE library_id = ?")
        .bind(lib_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert_eq!(
        facets.authors.len() as i64,
        FACET_LIMIT,
        "author facets must truncate at FACET_LIMIT when distinct authors exceed the cap"
    );
    assert!(
        facets
            .authors
            .windows(2)
            .all(|w| w[0].count == w[1].count && w[0].value < w[1].value),
        "surviving rows must stay ordered by count desc, value asc"
    );
    let expected_last = format!("Author {:06}", min_id + FACET_LIMIT - 1);
    assert_eq!(
        facets.authors.last().unwrap().value,
        expected_last,
        "truncation must keep the lexicographically-first FACET_LIMIT authors"
    );
}

#[tokio::test]
async fn library_facets_genres_are_empty_until_a_book_is_given_some() {
    let (pool, _guard) = seed_discovery_fixture().await;

    // The fixture books carry tags but no genres — nothing scans a genre.
    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert!(facets.genres.is_empty());

    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = crate::books::list_books(&pool, "/lib").await.unwrap();
    for (book, genres) in
        books
            .iter()
            .zip([vec!["Horror", "Mystery"], vec!["Horror"], vec!["Adventure"]])
    {
        crate::metadata_overrides::merge_metadata_overrides(
            &pool,
            &book.unique_identifier.clone().unwrap(),
            &omnibus_shared::MetadataOverrides {
                genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
                ..Default::default()
            },
            user_id,
        )
        .await
        .unwrap();
    }

    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert_eq!(
        pairs(&facets.genres),
        vec![
            ("Horror".into(), 2),
            ("Adventure".into(), 1),
            ("Mystery".into(), 1),
        ],
        "count desc, then value asc — same ordering as every other facet"
    );
}

#[tokio::test]
async fn library_facets_genres_fold_case_variants_into_one_canonical_facet() {
    // The facet value has to agree with `get_genre_cloud`, which is where
    // the filter chips come from: two facets that each match half the shelf
    // would be a filter the user cannot get a whole answer out of.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let books = crate::books::list_books(&pool, "/lib").await.unwrap();
    for (book, genre) in books.iter().zip(["Sci-Fi", "sci-fi", "SCI-FI"]) {
        crate::metadata_overrides::merge_metadata_overrides(
            &pool,
            &book.unique_identifier.clone().unwrap(),
            &omnibus_shared::MetadataOverrides {
                genres: Some(vec![genre.to_string()]),
                ..Default::default()
            },
            user_id,
        )
        .await
        .unwrap();
    }

    let facets = library_facets(&pool, &["/lib"]).await.unwrap();
    assert_eq!(
        pairs(&facets.genres),
        vec![("Sci-Fi".into(), 3)],
        "one facet under the first-coined spelling, counting all three books"
    );
}
