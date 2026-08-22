//! Genres, which exist only as an override-layer field: the rows a merge
//! materializes, the ones a drop or delete reaps, what a later edit
//! preserves, and the per-book cap a bulk delta must respect.

use omnibus_shared::{BulkMetadataEdit, MetadataOverrides};

use crate::books::{get_book_by_uuid, list_books};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;
use super::{genre_row_names, seed_one_book_with_tags, with_genres};

#[tokio::test]
async fn get_book_returns_genres_only_from_the_override_layer() {
    let _covers = CoversTempDir::new("genres_read");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &["fiction"]).await;

    // Nothing scans a genre, so a freshly indexed book has none even though
    // it does have a scanned tag.
    let before = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert!(before.genres.is_empty());
    assert_eq!(before.subjects, vec!["fiction".to_string()]);

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Horror"]), user_id)
        .await
        .unwrap();

    let after = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(after.genres, vec!["Horror".to_string()]);
    assert_eq!(
        after.subjects,
        vec!["fiction".to_string()],
        "a genres override leaves the scanned tags alone"
    );
}

#[tokio::test]
async fn merge_metadata_overrides_materializes_a_genres_row_per_entry() {
    let _covers = CoversTempDir::new("genres_materialize");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Horror", "Noir"]), user_id)
        .await
        .unwrap();

    assert_eq!(genre_row_names(&pool).await, vec!["Horror", "Noir"]);
}

#[tokio::test]
async fn merge_metadata_overrides_reaps_a_genre_orphaned_by_dropping_its_last_membership() {
    let _covers = CoversTempDir::new("genres_reap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Horror"]), user_id)
        .await
        .unwrap();
    assert_eq!(genre_row_names(&pool).await, vec!["Horror"]);

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Noir"]), user_id)
        .await
        .unwrap();
    assert_eq!(
        genre_row_names(&pool).await,
        vec!["Noir"],
        "the replaced genre's row is reaped in the same transaction"
    );
}

#[tokio::test]
async fn delete_metadata_overrides_reaps_the_genres_it_was_the_only_source_of() {
    let _covers = CoversTempDir::new("genres_reap_delete");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Horror"]), user_id)
        .await
        .unwrap();
    delete_metadata_overrides(&pool, &uuid).await.unwrap();

    assert!(
        genre_row_names(&pool).await.is_empty(),
        "reverting the override leaves no genre behind — there is no scanned \
         baseline for it to fall back to"
    );
    let book = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert!(book.genres.is_empty());
}

#[tokio::test]
async fn merge_metadata_overrides_preserves_genres_when_a_later_edit_omits_them() {
    let _covers = CoversTempDir::new("genres_preserve");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    merge_metadata_overrides(&pool, &uuid, &with_genres(&["Horror"]), user_id)
        .await
        .unwrap();
    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Retitled".into()),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let book = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(book.genres, vec!["Horror".to_string()]);
    assert_eq!(book.title.as_deref(), Some("Retitled"));
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_applies_genre_deltas_per_book() {
    let _covers = CoversTempDir::new("genres_bulk");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["Author"], &[], None, None),
            indexed("b.epub", Some("B"), &["Author"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let uuids: Vec<String> = books
        .iter()
        .map(|b| b.unique_identifier.clone().unwrap())
        .collect();

    // Seed one book with a genre the delta then removes, to prove the base
    // is the book's own current list rather than empty.
    merge_metadata_overrides(&pool, &uuids[0], &with_genres(&["Pulp"]), user_id)
        .await
        .unwrap();

    let edit = BulkMetadataEdit {
        add_genres: vec!["Horror".into()],
        remove_genres: vec!["Pulp".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &uuids, &edit, user_id)
        .await
        .unwrap();

    for uuid in &uuids {
        let book = get_book_by_uuid(&pool, uuid).await.unwrap().unwrap();
        assert_eq!(book.genres, vec!["Horror".to_string()], "book {uuid}");
    }
    assert_eq!(genre_row_names(&pool).await, vec!["Horror"]);
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_rejects_a_genre_delta_past_the_per_book_cap() {
    let _covers = CoversTempDir::new("genres_bulk_cap");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    // Seed the book at the cap, then add one more.
    let at_cap: Vec<String> = (0..MetadataOverrides::MAX_GENRES)
        .map(|i| format!("g{i}"))
        .collect();
    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            genres: Some(at_cap),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        add_genres: vec!["one-too-many".into()],
        ..Default::default()
    };
    let err = bulk_merge_metadata_overrides(&pool, std::slice::from_ref(&uuid), &edit, user_id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        MetadataOverridesError::TooManyValues { uuid: u, field: "genre", max }
            if u == uuid && max == MetadataOverrides::MAX_GENRES
    ));
}
