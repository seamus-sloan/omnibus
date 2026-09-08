//! The genres arm, sourced from the override JSON rather than a link
//! table: scoped counts, one row per canonical spelling, a repeated genre
//! counted once per book, the uncapped group total, and a book dropping
//! out when its genres are cleared.

use omnibus_shared::MetadataOverrides;
use sqlx::SqlitePool;

use super::super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

/// Seed `/lib` with one book per entry in `assignments` and give each the
/// genres named alongside it, through the override door — genres have no
/// other storage (migration `0066`). Returns the id of the user the overrides
/// are attributed to, so a test can re-assign without re-seeding the books.
async fn seed_genres(pool: &SqlitePool, assignments: &[(&str, &[&str])]) -> i64 {
    let books: Vec<_> = assignments
        .iter()
        .enumerate()
        .map(|(i, (title, _))| {
            indexed(
                &format!("{i}.epub"),
                Some(title),
                &["Author"],
                &[],
                None,
                None,
            )
        })
        .collect();
    replace_books(pool, "/lib", books).await.unwrap();

    let user_id = crate::auth::create_user(pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    set_genres(pool, user_id, assignments).await;
    user_id
}

/// Replace each named book's genre list through the override door.
async fn set_genres(pool: &SqlitePool, user_id: i64, assignments: &[(&str, &[&str])]) {
    let seeded = list_books(pool, "/lib").await.unwrap();
    for (title, genres) in assignments {
        let uuid = seeded
            .iter()
            .find(|b| b.title.as_deref() == Some(*title))
            .and_then(|b| b.unique_identifier.clone())
            .expect("seeded book");
        upsert_metadata_overrides(
            pool,
            &uuid,
            &MetadataOverrides {
                genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn search_genres_returns_matching_genre_with_scoped_count() {
    let _covers = CoversTempDir::new("arm_search_genres");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_genres(
        &pool,
        &[
            ("A", &["Dark academia"]),
            ("B", &["Dark academia"]),
            ("C", &["Cozy"]),
        ],
    )
    .await;

    let hits = search_genres(&pool, "/lib", "%academia%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].name, "Dark academia");
    assert_eq!(hits[0].book_count, 2);
}

#[tokio::test]
async fn search_genres_reports_one_row_under_the_canonical_spelling() {
    // The palette row has to agree with `get_genre_cloud` and the landing
    // facets: two rows splitting the same genre would each carry half the
    // count, and refining on one would answer only half the question.
    let _covers = CoversTempDir::new("arm_genres_canonical");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_genres(
        &pool,
        &[("A", &["Sci-Fi"]), ("B", &["sci-fi"]), ("C", &["SCI-FI"])],
    )
    .await;

    let hits = search_genres(&pool, "/lib", "%sci%", LIMIT).await.unwrap();
    assert_eq!(hits.len(), 1, "one row, under the first-coined spelling");
    assert_eq!(hits[0].name, "Sci-Fi");
    assert_eq!(hits[0].book_count, 3, "counting all three books");
    assert_eq!(count_genres(&pool, "/lib", "%sci%").await.unwrap(), 1);
}

#[tokio::test]
async fn search_genres_counts_a_repeated_genre_in_one_book_once() {
    let _covers = CoversTempDir::new("arm_genres_dupe");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_genres(&pool, &[("A", &["Horror", "Horror"])]).await;

    let hits = search_genres(&pool, "/lib", "%horror%", LIMIT)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].book_count, 1, "COUNT(DISTINCT b.id), not row count");
}

#[tokio::test]
async fn count_genres_counts_visible_matches_scoped() {
    let _covers = CoversTempDir::new("arm_count_genres");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_genres(
        &pool,
        &[
            ("A", &["match-one"]),
            ("B", &["match-two"]),
            ("C", &["other"]),
        ],
    )
    .await;

    assert_eq!(count_genres(&pool, "/lib", "%match-%").await.unwrap(), 2);
    assert_eq!(count_genres(&pool, "/lib", "%zzznope%").await.unwrap(), 0);
    // A genre nothing under this path names is invisible even though its
    // `genres` vocabulary row still exists.
    assert_eq!(count_genres(&pool, "/other", "%match-%").await.unwrap(), 0);
}

#[tokio::test]
async fn search_palette_groups_genres_with_an_uncapped_total() {
    // The group is capped at LIMIT hits but `genre_total` is not, so the
    // results header can say how many actually matched.
    let _covers = CoversTempDir::new("palette_genres_total");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let assignments: Vec<(String, Vec<String>)> = (0..7)
        .map(|i| (format!("Book {i}"), vec![format!("match-{i}")]))
        .collect();
    let borrowed: Vec<(&str, Vec<&str>)> = assignments
        .iter()
        .map(|(t, g)| (t.as_str(), g.iter().map(String::as_str).collect()))
        .collect();
    let refs: Vec<(&str, &[&str])> = borrowed.iter().map(|(t, g)| (*t, g.as_slice())).collect();
    seed_genres(&pool, &refs).await;

    let results = search_palette(&pool, "/lib", "match-").await.unwrap();
    assert_eq!(results.genres.len(), LIMIT as usize, "display cap applies");
    assert_eq!(results.genre_total, 7, "the total is uncapped");
    assert!(
        results.total_count() >= 7,
        "genre_total must feed total_count"
    );
}

#[tokio::test]
async fn search_palette_genres_drop_a_book_whose_genres_were_cleared() {
    let _covers = CoversTempDir::new("palette_genres_cleared");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = seed_genres(&pool, &[("A", &["Horror"]), ("B", &["Horror"])]).await;
    assert_eq!(
        search_palette(&pool, "/lib", "horror")
            .await
            .unwrap()
            .genres[0]
            .book_count,
        2
    );

    // Clearing one book's list drops it from the count; clearing the other's
    // removes the row entirely, since nothing live names the genre any more.
    set_genres(&pool, user_id, &[("A", &[])]).await;
    let results = search_palette(&pool, "/lib", "horror").await.unwrap();
    assert_eq!(results.genres.len(), 1);
    assert_eq!(results.genres[0].book_count, 1);

    set_genres(&pool, user_id, &[("B", &[])]).await;
    let results = search_palette(&pool, "/lib", "horror").await.unwrap();
    assert!(
        results.genres.is_empty(),
        "a genre no live book names must not render"
    );
    assert_eq!(results.genre_total, 0);
}
