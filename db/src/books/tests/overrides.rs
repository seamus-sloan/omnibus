//! How `metadata_overrides` rows merge into `get_book` and `list_books`:
//! scalar overrides, whole-list replacement of creators and subjects, the
//! creator/series id backfills, and the overrides-JSON error variant.

use omnibus_shared::{Contributor, MetadataOverrides};

use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{
    author_id_by_name, indexed, seed_discovery_fixture, series_id_by_name, CoversTempDir,
};

use super::super::*;

#[tokio::test]
async fn get_book_merges_scalar_overrides() {
    let _covers = CoversTempDir::new("merge_scalar");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "merge.epub",
            Some("Original Title"),
            &["Author A"],
            &["fiction"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let book = &books[0];
    let uuid = book.unique_identifier.clone().unwrap();
    let id = book.id;

    // Save overrides.
    let ov = MetadataOverrides {
        title: Some("Edited Title".into()),
        publisher: Some("New Publisher".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    // get_book should return merged values.
    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.title.as_deref(), Some("Edited Title"));
    assert_eq!(merged.publisher.as_deref(), Some("New Publisher"));
    assert!(merged.has_override);
    // Non-overridden fields unchanged.
    assert_eq!(merged.creators[0].name, "Author A");
}

#[tokio::test]
async fn get_book_merges_creators_replaces_entirely() {
    let _covers = CoversTempDir::new("merge_creators");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "creators.epub",
            Some("Book"),
            &["Author A"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let id = books[0].id;

    let ov = MetadataOverrides {
        creators: Some(vec![
            Contributor {
                name: "Author B".into(),
                ..Default::default()
            },
            Contributor {
                name: "Author C".into(),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 2);
    assert_eq!(merged.creators[0].name, "Author B");
    assert_eq!(merged.creators[1].name, "Author C");
}

#[tokio::test]
async fn get_book_merges_subjects_replaces_entirely() {
    let _covers = CoversTempDir::new("merge_subjects");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "subjects.epub",
            Some("Book"),
            &["Author"],
            &["fiction", "classic"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let id = books[0].id;

    let ov = MetadataOverrides {
        subjects: Some(vec!["sci-fi".into(), "adventure".into()]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.subjects, vec!["sci-fi", "adventure"]);
}

#[tokio::test]
async fn get_book_backfills_creator_ids_after_override_replaces_authors() {
    // Override Contributors carry only a name, so a book whose author
    // list was edited through the metadata form would otherwise come
    // back with `creators[*].id == None`, rendering the breadcrumb's
    // author link as an unclickable span even when the `authors` row
    // exists. Verify get_book backfills the id by name. Mirrors the
    // user's report against book 268 (multi-author book where the
    // user removed all but one canonical author).
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    // saga1.epub canonically has ["Ada Lovelace", "Grace Hopper"];
    // simulate the user dropping the second author through the edit
    // form. apply_overrides replaces creators wholesale, so the
    // override Contributor has id = None.
    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Ada Lovelace".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "Ada Lovelace");
    assert_eq!(
        merged.creators[0].id,
        Some(ada_id),
        "creator id must be backfilled so the breadcrumb renders as a Link",
    );
}

#[tokio::test]
async fn get_book_backfills_creator_ids_case_insensitively() {
    // `authors.name` is `UNIQUE COLLATE NOCASE`, so a SQL `IN (...)`
    // lookup matches case-insensitively — but the returned row carries
    // the DB casing while the override carries the user-supplied
    // casing. The HashMap must normalise both sides to lowercase so
    // an override like "ada lovelace" still resolves to the canonical
    // "Ada Lovelace" id.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "ADA LOVELACE".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "ADA LOVELACE");
    assert_eq!(
        merged.creators[0].id,
        Some(ada_id),
        "case-mismatched override should still resolve to the canonical author id",
    );
}

#[tokio::test]
async fn get_book_leaves_creator_id_none_when_override_author_unknown() {
    // If the override sets an author name that doesn't exist in the
    // `authors` table, backfill must leave the id None.
    let (pool, _guard) = seed_discovery_fixture().await;
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let books = list_books(&pool, "/lib").await.unwrap();
    let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
    let uuid = saga_one.unique_identifier.clone().unwrap();
    let book_id = saga_one.id;

    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Nobody Indexed".into(),
            role: Some("aut".into()),
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, book_id).await.unwrap().unwrap();
    assert_eq!(merged.creators.len(), 1);
    assert_eq!(merged.creators[0].name, "Nobody Indexed");
    assert_eq!(merged.creators[0].id, None);
}

#[tokio::test]
async fn get_book_backfills_series_id_from_override_when_series_exists() {
    // A book whose series was set via overrides (not at scan time)
    // historically came back with series_id == None even though the
    // series row existed in the relational table. The detail page's
    // "Series" rail then fell back to plain text instead of a Link
    // to /series/:id. Verify the read path now backfills the id.
    let _covers = CoversTempDir::new("override_series_link");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Seed: one book belongs to "Saga" natively (so the series row exists),
    // one standalone book that we'll later override into the same series.
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "saga1.epub",
                Some("Saga: Book One"),
                &["Author X"],
                &[],
                Some(("Saga", "1")),
                None,
            ),
            indexed("loner.epub", Some("Loner"), &["Author Y"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let saga_id = series_id_by_name(&pool, "Saga").await;
    let books = list_books(&pool, "/lib").await.unwrap();
    let loner = books.iter().find(|b| b.filename == "loner.epub").unwrap();
    assert_eq!(loner.series, None);
    assert_eq!(loner.series_id, None);
    let loner_uuid = loner.unique_identifier.clone().unwrap();
    let loner_book_id = loner.id;

    // Override the standalone to be part of "Saga". The overrides path
    // does not touch books_series_link, so loner.series_id stays unset
    // in the relational table — get_book must backfill from the series
    // table by name.
    let ov = MetadataOverrides {
        series: Some("Saga".into()),
        series_index: Some("3".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &loner_uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, loner_book_id).await.unwrap().unwrap();
    assert_eq!(merged.series.as_deref(), Some("Saga"));
    assert_eq!(
        merged.series_id,
        Some(saga_id),
        "override-only series must still resolve series_id so the detail rail can link"
    );
}

#[tokio::test]
async fn get_book_populates_series_id_when_override_creates_series() {
    // When an override sets a series name, `upsert_metadata_overrides`
    // materializes the `series` row + `books_series_link` so the
    // detail-page breadcrumb is clickable and `/series` lists it.
    let _covers = CoversTempDir::new("override_series_unknown");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "alone.epub",
            Some("Alone"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let book = &books[0];
    let uuid = book.unique_identifier.clone().unwrap();
    let id = book.id;

    let ov = MetadataOverrides {
        series: Some("A Series That Does Not Yet Exist".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        merged.series.as_deref(),
        Some("A Series That Does Not Yet Exist")
    );
    assert!(
        merged.series_id.is_some(),
        "override should materialize series row so breadcrumb is clickable"
    );
}

#[tokio::test]
async fn list_books_merges_overrides_in_bulk() {
    let _covers = CoversTempDir::new("bulk_merge");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("Book A"), &["Author A"], &[], None, None),
            indexed("b.epub", Some("Book B"), &["Author B"], &[], None, None),
            indexed("c.epub", Some("Book C"), &["Author C"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid_a = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book A"))
        .unwrap()
        .unique_identifier
        .clone()
        .unwrap();
    let uuid_c = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book C"))
        .unwrap()
        .unique_identifier
        .clone()
        .unwrap();

    // Override A and C only.
    upsert_metadata_overrides(
        &pool,
        &uuid_a,
        &MetadataOverrides {
            title: Some("Edited A".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    upsert_metadata_overrides(
        &pool,
        &uuid_c,
        &MetadataOverrides {
            title: Some("Edited C".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let a = books
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&uuid_a))
        .unwrap();
    let b = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Book B"))
        .unwrap();
    let c = books
        .iter()
        .find(|b| b.unique_identifier.as_deref() == Some(&uuid_c))
        .unwrap();

    assert_eq!(a.title.as_deref(), Some("Edited A"));
    assert!(a.has_override);
    assert_eq!(b.title.as_deref(), Some("Book B"));
    assert!(!b.has_override);
    assert_eq!(c.title.as_deref(), Some("Edited C"));
    assert!(c.has_override);
}

// ---------- BooksError variants ----------

#[test]
fn books_error_maps_overrides_serialization_to_overrides_json_variant() {
    // The `From<MetadataOverridesError>` bridge (the only site that mints
    // `BooksError::OverridesJson`) must route a corrupt-overrides-JSON
    // deserialization failure to `OverridesJson`, carrying the underlying
    // `serde_json::Error` and its message — never collapsing it into `Db`.
    let json_err =
        serde_json::from_str::<MetadataOverrides>("{ not valid json").expect_err("must not parse");
    let src = crate::metadata_overrides::MetadataOverridesError::Serialization(json_err);
    let err: BooksError = src.into();
    assert!(
        matches!(err, BooksError::OverridesJson(_)),
        "Serialization must map to OverridesJson, got {err:?}"
    );
    assert!(
        err.to_string()
            .starts_with("overrides deserialization failed"),
        "got {err}"
    );
}
