//! The arm functions called directly with an already-escaped `%pattern%`
//! (they are normally reached only through `search_palette`, which escapes
//! LIKE metacharacters first): `search_*` rows, limits, and the scoped,
//! uncapped `count_*` twins.

use super::super::*;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

// The arm functions below are normally reached only through
// `search_palette`; these exercise them directly with an already-escaped
// `%pattern%` (the caller escapes LIKE metacharacters before calling).
#[tokio::test]
async fn search_authors_returns_matching_author_with_scoped_count() {
    let _covers = CoversTempDir::new("arm_search_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Babel"), &["R. F. Kuang"], &[], None, None),
            indexed(
                "b.epub",
                Some("Yellowface"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            ),
            indexed("c.epub", Some("Other"), &["Someone Else"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_authors(&pool, "/lib", "%kuang%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1, "only the Kuang author matches");
    assert_eq!(hits[0].name, "R. F. Kuang");
    assert_eq!(hits[0].book_count, 2);
    assert_eq!(hits[0].lead_book_title.as_deref(), Some("Babel"));
}

#[tokio::test]
async fn search_authors_respects_limit() {
    let _covers = CoversTempDir::new("arm_search_authors_limit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let books: Vec<_> = (0..4)
        .map(|i| {
            indexed(
                &format!("m{i}.epub"),
                Some(&format!("Book {i}")),
                &[&format!("Match Author {i}")],
                &[],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();

    let hits = search_authors(&pool, "/lib", "%match%", 2).await.unwrap();
    assert_eq!(hits.len(), 2, "arm caps at the supplied limit");
}

#[tokio::test]
async fn count_authors_counts_visible_matches_uncapped() {
    let _covers = CoversTempDir::new("arm_count_authors");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let books: Vec<_> = (0..7)
        .map(|i| {
            indexed(
                &format!("m{i}.epub"),
                Some(&format!("Book {i}")),
                &[&format!("Match Author {i}")],
                &[],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();

    // Uncapped: counts all 7 matching authors regardless of the display cap.
    let total = count_authors(&pool, "/lib", "%match%").await.unwrap();
    assert_eq!(total, 7);
    // A non-matching pattern counts zero.
    assert_eq!(count_authors(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}

#[tokio::test]
async fn count_authors_is_scoped_to_library() {
    let _covers = CoversTempDir::new("arm_count_authors_scoped");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib-a",
        vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib-b",
        vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
    )
    .await
    .unwrap();

    // Same author name in both libraries, but the count is per-library.
    assert_eq!(
        count_authors(&pool, "/lib-a", "%tolkien%").await.unwrap(),
        1
    );
}

#[tokio::test]
async fn search_series_returns_matching_series_with_author_display() {
    let _covers = CoversTempDir::new("arm_search_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Book One"),
                &["Author X"],
                &[],
                Some(("Poppy War", "1")),
                None,
            ),
            indexed(
                "b.epub",
                Some("Book Two"),
                &["Author X"],
                &[],
                Some(("Poppy War", "2")),
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let hits = search_series(&pool, "/lib", "%poppy%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Poppy War");
    assert_eq!(hits[0].book_count, 2);
    assert_eq!(hits[0].author_display.as_deref(), Some("Author X"));
    assert_eq!(hits[0].lead_book_title.as_deref(), Some("Book One"));
}

#[tokio::test]
async fn count_series_counts_visible_matches_scoped() {
    let _covers = CoversTempDir::new("arm_count_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("A"),
                &["X"],
                &[],
                Some(("Match Alpha", "1")),
                None,
            ),
            indexed(
                "b.epub",
                Some("B"),
                &["X"],
                &[],
                Some(("Match Beta", "1")),
                None,
            ),
            indexed(
                "c.epub",
                Some("C"),
                &["X"],
                &[],
                Some(("Unrelated", "1")),
                None,
            ),
        ],
    )
    .await
    .unwrap();

    assert_eq!(count_series(&pool, "/lib", "%match%").await.unwrap(), 2);
    assert_eq!(count_series(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}

#[tokio::test]
async fn search_tags_returns_matching_tag_with_scoped_count() {
    let _covers = CoversTempDir::new("arm_search_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("B"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            ),
            indexed("c.epub", Some("C"), &["Author"], &["Cozy"], None, None),
        ],
    )
    .await
    .unwrap();

    let hits = search_tags(&pool, "/lib", "%academia%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Dark academia");
    assert_eq!(hits[0].book_count, 2);
}

#[tokio::test]
async fn count_tags_counts_visible_matches_scoped() {
    let _covers = CoversTempDir::new("arm_count_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &["match-one"], None, None),
            indexed("b.epub", Some("B"), &["X"], &["match-two"], None, None),
            indexed("c.epub", Some("C"), &["X"], &["other"], None, None),
        ],
    )
    .await
    .unwrap();

    assert_eq!(count_tags(&pool, "/lib", "%match-%").await.unwrap(), 2);
    assert_eq!(count_tags(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
}
