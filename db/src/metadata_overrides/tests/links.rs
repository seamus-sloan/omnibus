//! Series, tag, and genre link materialization: the rows an override
//! creates, the ones it reaps when it drops the last membership, the
//! canonical rows it must leave alone, and the chunking that keeps a name
//! list past SQLite's bind cap intact.

use omnibus_shared::MetadataOverrides;

use crate::books::{get_book, list_books};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;
use super::{genre_row_names, seed_one_book_with_tags, tag_row_count};

#[tokio::test]
async fn upsert_metadata_overrides_materializes_series_link_for_new_series() {
    let _covers = CoversTempDir::new("materialize_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    crate::sync::sync_audiobooks(
        &pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                "author/book",
                "My Audiobook",
                Some("Narrator"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let book = list_books(&pool, "/audio").await.unwrap();
    let uuid = book[0].unique_identifier.clone().unwrap();
    let id = book[0].id;

    assert!(
        get_book(&pool, id)
            .await
            .unwrap()
            .unwrap()
            .series_id
            .is_none(),
        "no series before override"
    );

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            series: Some("My Series".into()),
            series_index: Some("1".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(book.series.as_deref(), Some("My Series"));
    assert!(
        book.series_id.is_some(),
        "series_id should be set after override materializes the link"
    );
}

#[tokio::test]
async fn upsert_metadata_overrides_materializes_tag_links_for_new_tags() {
    let _covers = CoversTempDir::new("materialize_tags");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "book.epub",
            Some("Tagged Book"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let book = list_books(&pool, "/lib").await.unwrap();
    let uuid = book[0].unique_identifier.clone().unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["Brand New Tag".into(), "  ".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // `get_tag_cloud` selects FROM `tags`, so the override-created tag
    // appearing here proves the materialized row and its override membership
    // joined up — this is what feeds the inline-edit autocomplete pool.
    let cloud = crate::get_tag_cloud(&pool).await.unwrap();
    assert!(
        cloud.iter().any(|t| t.name == "Brand New Tag"),
        "override-created tag should appear in the tag cloud, got: {:?}",
        cloud.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    // The blank entry is trimmed/skipped, never materialized.
    assert!(
        cloud.iter().all(|t| !t.name.trim().is_empty()),
        "blank subjects must not materialize tag rows"
    );
}

#[tokio::test]
async fn upsert_metadata_overrides_deletes_a_tag_orphaned_by_dropping_its_last_membership() {
    let _covers = CoversTempDir::new("reap_tag_upsert");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    let with_subjects = |subjects: &[&str]| MetadataOverrides {
        subjects: Some(subjects.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    };
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &with_subjects(&["keep", "drop"]),
        false,
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(tag_row_count(&pool, "drop").await, 1);

    upsert_metadata_overrides(&pool, &uuid, &with_subjects(&["keep"]), false, user_id)
        .await
        .unwrap();
    assert_eq!(
        tag_row_count(&pool, "drop").await,
        0,
        "the save dropped the tag's last membership, so the row must be reaped"
    );
    assert_eq!(tag_row_count(&pool, "keep").await, 1);
}

#[tokio::test]
async fn upsert_metadata_overrides_keeps_canonical_tag_rows_shadowed_by_an_override() {
    let _covers = CoversTempDir::new("reap_tag_shadowed");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, id) = seed_one_book_with_tags(&pool, &["scanned"]).await;

    // Clear-all override: the tag's every *effective* membership is gone,
    // but its canonical link row is the scanned truth — reaping it would
    // make revert-to-scanned lossy.
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec![]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(tag_row_count(&pool, "scanned").await, 1);
    let links: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_tags_link")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(links, 1, "the canonical link row must survive the override");

    delete_metadata_overrides(&pool, &uuid).await.unwrap();
    let reverted = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        reverted.subjects,
        vec!["scanned"],
        "revert-to-scanned must still restore the tag"
    );
}

#[tokio::test]
async fn delete_metadata_overrides_deletes_tags_that_existed_only_through_the_override() {
    let _covers = CoversTempDir::new("reap_tag_delete");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["solo".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(tag_row_count(&pool, "solo").await, 1);

    delete_metadata_overrides(&pool, &uuid).await.unwrap();
    assert_eq!(
        tag_row_count(&pool, "solo").await,
        0,
        "reverting to scanned must reap the override-only tag"
    );
}

#[tokio::test]
async fn merge_metadata_overrides_deletes_a_tag_dropped_by_the_replacing_subjects_list() {
    let _covers = CoversTempDir::new("reap_tag_merge");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["a".into(), "b".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // `merge` replaces m2m fields wholesale when `Some` — "b" loses its
    // only membership.
    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["a".into()]),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(tag_row_count(&pool, "b").await, 0, "dropped tag reaped");
    assert_eq!(tag_row_count(&pool, "a").await, 1);
}

/// The override row and the `books_series_link` row must land
/// atomically: after `upsert_metadata_overrides` commits, a direct
/// SELECT on `books_series_link` must already reflect the new series.
/// Guards the post-#576 invariant that the link materialize runs inside
/// the same transaction as the override row, so the book detail page
/// never observes a fresh override against a stale link.
#[tokio::test]
async fn upsert_metadata_overrides_persists_books_series_link_row_for_new_series() {
    let _covers = CoversTempDir::new("series_link_row");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "link.epub",
            Some("A Book"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let book_id = books[0].id;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            series: Some("Linked Series".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let linked_series: Option<String> = sqlx::query_scalar(
        "SELECT s.name
           FROM books_series_link bsl
           JOIN series s ON s.id = bsl.series
          WHERE bsl.book = ?",
    )
    .bind(book_id)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(
        linked_series.as_deref(),
        Some("Linked Series"),
        "books_series_link should reflect the new override series after upsert commits"
    );
}

// -----------------------------------------------------------------
// Batched tag / genre vocabulary materialization
// -----------------------------------------------------------------

/// A name list deliberately past SQLite's 999 bound-parameter cap, so an
/// unchunked single-statement insert would error instead of inserting.
fn oversized_name_list(prefix: &str) -> Vec<String> {
    (0..1_500).map(|i| format!("{prefix}-{i}")).collect()
}

#[tokio::test]
async fn materialize_tag_rows_inserts_every_name_when_the_list_exceeds_the_sqlite_bind_cap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let names = oversized_name_list("tag");
    let ov = MetadataOverrides {
        subjects: Some(names.clone()),
        ..Default::default()
    };
    let mut conn = pool.acquire().await.unwrap();

    super::super::links::materialize_tag_rows(&mut conn, &ov)
        .await
        .unwrap();
    // Re-running must stay a no-op: `INSERT OR IGNORE` survives batching.
    super::super::links::materialize_tag_rows(&mut conn, &ov)
        .await
        .unwrap();
    drop(conn);

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        count,
        names.len() as i64,
        "every name in an over-the-cap list should be materialized exactly once"
    );
    assert_eq!(tag_row_count(&pool, "tag-1499").await, 1);
}

#[tokio::test]
async fn materialize_genre_rows_inserts_every_name_when_the_list_exceeds_the_sqlite_bind_cap() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let names = oversized_name_list("genre");
    let ov = MetadataOverrides {
        genres: Some(names.clone()),
        ..Default::default()
    };
    let mut conn = pool.acquire().await.unwrap();

    super::super::links::materialize_genre_rows(&mut conn, &ov)
        .await
        .unwrap();
    super::super::links::materialize_genre_rows(&mut conn, &ov)
        .await
        .unwrap();
    drop(conn);

    assert_eq!(genre_row_names(&pool).await.len(), names.len());
}

#[tokio::test]
async fn upsert_metadata_overrides_materializes_each_tag_and_genre_once_when_names_repeat() {
    let _covers = CoversTempDir::new("batched_vocab");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let (uuid, _) = seed_one_book_with_tags(&pool, &[]).await;

    // Repeats, casing variants, padding and blanks in one save — the batch
    // must collapse them exactly as the old per-name statements did.
    let ov = MetadataOverrides {
        subjects: Some(vec![
            "Space Opera".into(),
            " Space Opera ".into(),
            "space opera".into(),
            "  ".into(),
            "Hard SF".into(),
        ]),
        genres: Some(vec![
            "Horror".into(),
            "horror".into(),
            "Noir".into(),
            "".into(),
        ]),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    assert_eq!(tag_row_count(&pool, "Space Opera").await, 1);
    assert_eq!(tag_row_count(&pool, "Hard SF").await, 1);
    let tag_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        tag_count, 2,
        "blank and duplicate subjects never materialize"
    );
    assert_eq!(genre_row_names(&pool).await, vec!["Horror", "Noir"]);

    // Re-saving the same lists changes nothing.
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();
    assert_eq!(tag_row_count(&pool, "Space Opera").await, 1);
    assert_eq!(genre_row_names(&pool).await, vec!["Horror", "Noir"]);
}
