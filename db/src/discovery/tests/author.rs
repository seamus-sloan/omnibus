//! `get_author` and `get_author_for_paths`: books ordered by series index
//! with series ids populated, the missing-id and DB-failure paths, library
//! scoping, the discovery cap, `has_photo`, and an empty series index
//! sorting last.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use crate::author_photos_data::{upsert_author_photo, AuthorPhotoSource};
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{
    author_id_by_name, indexed, seed_books_for_one_author_and_series, seed_discovery_fixture,
    series_id_by_name, CoversTempDir,
};

#[tokio::test]
async fn get_author_returns_author_with_all_books_ordered_by_series_index() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let author = get_author(&pool, id).await.unwrap().expect("author exists");

    assert_eq!(author.name, "Ada Lovelace");
    assert_eq!(author.book_count, 3);
    assert_eq!(author.books.len(), 3);

    // Series books come first, ordered by series_index ASC (NULLS LAST
    // means the standalone trails).
    let titles: Vec<_> = author
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert_eq!(
        titles,
        vec![
            "Saga: Book One".to_string(),
            "Saga: Book Two".to_string(),
            "Standalone".to_string(),
        ]
    );
}

#[tokio::test]
async fn get_author_populates_series_id_on_books() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;
    let expected_sid = series_id_by_name(&pool, "Saga").await;

    let author = get_author(&pool, id).await.unwrap().unwrap();
    for book in author.books.iter().filter(|b| b.series.is_some()) {
        assert_eq!(
            book.series_id,
            Some(expected_sid),
            "series book should carry series_id"
        );
    }
    let standalone = author
        .books
        .iter()
        .find(|b| b.series.is_none())
        .expect("standalone present");
    assert_eq!(standalone.series_id, None);
}

#[tokio::test]
async fn get_author_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_author(&pool, 999_999).await.unwrap();
    assert!(missing.is_none());
}

/// Add one book by Ada Lovelace under a second scan root, so the fixture
/// has an author whose books straddle two libraries.
async fn seed_second_library_book(pool: &sqlx::SqlitePool) {
    replace_books(
        pool,
        "/other",
        vec![indexed(
            "elsewhere.epub",
            Some("Elsewhere"),
            &["Ada Lovelace"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn get_author_for_paths_excludes_books_indexed_under_another_library() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &["/lib"])
        .await
        .unwrap()
        .expect("author exists");

    let titles: Vec<_> = scoped
        .books
        .iter()
        .filter_map(|b| b.title.clone())
        .collect();
    assert!(!titles.contains(&"Elsewhere".to_string()));
    assert_eq!(scoped.books.len(), 3);
    assert_eq!(scoped.book_count, 3, "book_count follows the same scope");
}

#[tokio::test]
async fn get_author_for_paths_includes_books_from_every_listed_library() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &["/lib", "/other"])
        .await
        .unwrap()
        .expect("author exists");

    assert_eq!(scoped.books.len(), 4);
    assert_eq!(scoped.book_count, 4);
}

#[tokio::test]
async fn get_author_for_paths_returns_no_books_for_an_empty_path_list() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let scoped = get_author_for_paths(&pool, id, &[])
        .await
        .unwrap()
        .expect("author exists");

    assert!(scoped.books.is_empty());
    assert_eq!(scoped.book_count, 0);
}

#[tokio::test]
async fn get_author_returns_books_from_every_library_when_unscoped() {
    let (pool, _guard) = seed_discovery_fixture().await;
    seed_second_library_book(&pool).await;
    let id = author_id_by_name(&pool, "Ada Lovelace").await;

    let author = get_author(&pool, id).await.unwrap().expect("author exists");

    assert_eq!(author.books.len(), 4);
    assert_eq!(author.book_count, 4);
}

#[tokio::test]
async fn get_author_for_paths_returns_none_for_missing_id() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let missing = get_author_for_paths(&pool, 999_999, &["/lib"])
        .await
        .unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn get_author_for_paths_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_author_for_paths(&pool, 1, &["/lib"]).await.unwrap_err();
    assert!(matches!(err, DiscoveryError::Db(_)));
}

#[tokio::test]
async fn get_author_caps_books_at_max_discovery_books() {
    let _covers = CoversTempDir::new("author_cap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let total = MAX_DISCOVERY_BOOKS + 25;
    let (author_id, _series_id) = seed_books_for_one_author_and_series(&pool, total).await;

    let author = get_author(&pool, author_id)
        .await
        .unwrap()
        .expect("author exists");
    assert_eq!(
        author.books.len() as i64,
        MAX_DISCOVERY_BOOKS,
        "get_author must cap the nested books vec at MAX_DISCOVERY_BOOKS"
    );
    assert_eq!(
        author.book_count as i64, total,
        "book_count must report the true (uncapped) shelf size"
    );
    assert!(
        author.book_count > author.books.len(),
        "truncation must be detectable as book_count > books.len()"
    );
}

#[tokio::test]
async fn get_author_empty_string_series_index_sorts_last() {
    // Mirror of get_series: clearing the position field (`Some("")`)
    // used to CAST('') to 0.0 in get_author's ORDER BY and sort the
    // book to the front of the author's shelf. NULLIF drops it to NULL
    // so NULLS LAST trails it behind positioned books.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let author_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = book_one.unique_identifier.clone().unwrap();

    // Keep Book One (canonical Saga #1) but clear its position.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some(String::new()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let author = get_author(&pool, author_id)
        .await
        .unwrap()
        .expect("author exists");
    let titles: Vec<_> = author
        .books
        .iter()
        .map(|b| b.title.clone().unwrap_or_default())
        .collect();
    let pos = |t: &str| titles.iter().position(|x| x == t).unwrap();
    assert!(
        pos("Saga: Book Two") < pos("Saga: Book One"),
        "cleared series_index should trail the positioned book, got {titles:?}",
    );
    assert_ne!(
        titles.first().map(String::as_str),
        Some("Saga: Book One"),
        "cleared series_index must not sort to the front, got {titles:?}",
    );
}

#[tokio::test]
async fn get_author_populates_has_photo() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    // No row → false.
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(!ada.has_photo, "no row should yield has_photo = false");

    // Letter marker → still false (negative-cache shouldn't render an img).
    upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(
        !ada.has_photo,
        "letter marker should yield has_photo = false"
    );

    // Manual upload → true.
    upsert_author_photo(
        &pool,
        ada_id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(b"\xFF\xD8\xFFfake"),
    )
    .await
    .unwrap();
    let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
    assert!(ada.has_photo, "manual upload should yield has_photo = true");
}
