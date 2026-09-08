//! `get_series`: books ordered by series index, the missing-id path, the
//! discovery cap, a pinned series id for books moved between series, and
//! an empty series index sorting last.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::test_support::{
    seed_books_for_one_author_and_series, seed_discovery_fixture, series_id_by_name, CoversTempDir,
};

#[tokio::test]
async fn get_series_returns_books_ordered_by_series_index() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = series_id_by_name(&pool, "Saga").await;

    let series = get_series(&pool, id).await.unwrap().expect("series exists");
    assert_eq!(series.name, "Saga");
    assert_eq!(series.book_count, 2);

    let titles: Vec<_> = series
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec!["Saga: Book One".to_string(), "Saga: Book Two".to_string()]
    );
    // Each book should carry the parent series id back out so the
    // frontend can navigate cross-references without an extra lookup.
    for book in &series.books {
        assert_eq!(book.series_id, Some(id));
    }
}

#[tokio::test]
async fn get_series_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_series(&pool, 999_999).await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn get_series_caps_books_at_max_discovery_books() {
    let _covers = CoversTempDir::new("series_cap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_DISCOVERY_BOOKS + 25;
    let (_author_id, series_id) = seed_books_for_one_author_and_series(&pool, total).await;

    let series = get_series(&pool, series_id)
        .await
        .unwrap()
        .expect("series exists");
    assert_eq!(
        series.books.len() as i64,
        MAX_DISCOVERY_BOOKS,
        "get_series must cap the nested books vec at MAX_DISCOVERY_BOOKS"
    );
    assert_eq!(
        series.book_count as i64, total,
        "book_count must report the true (uncapped) series size"
    );
    assert!(
        series.book_count > series.books.len(),
        "truncation must be detectable as book_count > books.len()"
    );
}

#[tokio::test]
async fn get_series_empty_string_series_index_sorts_last() {
    // `Some("")` from the edit form (user cleared the position
    // field) was sorting to the front because `CAST('' AS REAL)`
    // returns 0.0. NULLIF on the override value drops it to NULL,
    // and ORDER BY ... NULLS LAST trails it after positioned books.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let saga_id = series_id_by_name(&pool, "Saga").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let standalone = books
        .iter()
        .find(|b| b.filename == "standalone.epub")
        .unwrap();
    let uuid = standalone.unique_identifier.clone().unwrap();

    // Add Standalone to Saga but clear its position.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some(String::new()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let series = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("series exists");
    let titles: Vec<_> = series
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Saga: Book One".to_string(),
            "Saga: Book Two".to_string(),
            "Standalone".to_string(),
        ],
        "empty-string series_index should trail positioned books, not lead them",
    );
}

#[tokio::test]
async fn get_series_pins_series_id_for_books_moved_between_series() {
    // A book canonically in Series A overridden into Series B used
    // to come back from get_series(B) with `series_id = Some(A)`
    // (BOOK_COLUMNS reads only books_series_link), so the card on
    // B's page would link back to /series/A. The fix pins
    // series_id/series unconditionally to the requested parent.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let pioneers_id = series_id_by_name(&pool, "Pioneers").await;

    // "Other Story" is canonically in Pioneers; override moves it
    // into Saga. Verify that opening Saga's page returns the book
    // pinned to Saga's id, not Pioneers'.
    let books = list_books(&pool, "/lib").await.unwrap();
    let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
    let uuid = other.unique_identifier.clone().unwrap();

    let saga_id = series_id_by_name(&pool, "Saga").await;
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("5".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let saga = get_series(&pool, saga_id)
        .await
        .unwrap()
        .expect("Saga exists");
    let moved = saga
        .books
        .iter()
        .find(|b| b.title.as_deref() == Some("Other Story"))
        .expect("override moved Other Story into Saga");
    assert_eq!(
        moved.series_id,
        Some(saga_id),
        "card on Saga's page must link back to Saga, not the canonical Pioneers",
    );
    assert_eq!(moved.series.as_deref(), Some("Saga"));

    // And it should be gone from Pioneers' page.
    let pioneers = get_series(&pool, pioneers_id)
        .await
        .unwrap()
        .expect("Pioneers exists");
    assert!(
        !pioneers
            .books
            .iter()
            .any(|b| b.title.as_deref() == Some("Other Story")),
        "override moved Other Story off Pioneers",
    );
}
