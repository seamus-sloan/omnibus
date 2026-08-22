//! `bulk_merge_metadata_overrides`: the scalar and author fields it applies
//! to every book, the tag deltas it resolves against each book's own
//! effective subjects, the caps it enforces, and the all-or-nothing
//! rollback.

use omnibus_shared::{BulkMetadataEdit, MetadataOverrides};

use crate::books::{get_book, list_books};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;
use super::tag_row_count;

// -----------------------------------------------------------------
// Bulk merge (table-view bulk edit)
// -----------------------------------------------------------------

/// Seed two scanned books under `/lib` with distinct subjects and return
/// their `(uuid, id)` pairs keyed by filename order (a.epub, b.epub).
async fn seed_two_books_for_bulk(pool: &sqlx::SqlitePool) -> Vec<(String, i64)> {
    replace_books(
        pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Alpha"),
                &["Old Author"],
                &["scifi", "classic"],
                None,
                None,
            ),
            indexed(
                "b.epub",
                Some("Beta"),
                &["Old Author"],
                &["romance"],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();
    let mut books = list_books(pool, "/lib").await.unwrap();
    books.sort_by(|a, b| a.filename.cmp(&b.filename));
    books
        .iter()
        .map(|b| (b.unique_identifier.clone().unwrap(), b.id))
        .collect()
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_applies_scalars_and_authors_to_every_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let uuids: Vec<String> = seeded.iter().map(|(u, _)| u.clone()).collect();

    let edit = BulkMetadataEdit {
        authors: Some(vec!["New Author".into()]),
        publisher: Some("Bulk House".into()),
        series: Some("Bulk Series".into()),
        language: Some("en".into()),
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &uuids, &edit, user_id)
        .await
        .unwrap();

    for (_, id) in &seeded {
        let book = get_book(&pool, *id).await.unwrap().unwrap();
        assert_eq!(book.publisher, Some("Bulk House".into()));
        assert_eq!(book.series, Some("Bulk Series".into()));
        assert_eq!(book.language, Some("en".into()));
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, "New Author");
    }
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_adds_and_removes_tags_against_each_books_effective_subjects()
{
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let uuids: Vec<String> = seeded.iter().map(|(u, _)| u.clone()).collect();

    let edit = BulkMetadataEdit {
        add_tags: vec!["fantasy".into()],
        remove_tags: vec!["scifi".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &uuids, &edit, user_id)
        .await
        .unwrap();

    let alpha = get_book(&pool, seeded[0].1).await.unwrap().unwrap();
    let beta = get_book(&pool, seeded[1].1).await.unwrap().unwrap();
    assert_eq!(alpha.subjects, vec!["classic", "fantasy"]);
    assert_eq!(beta.subjects, vec!["romance", "fantasy"]);
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_uses_existing_override_subjects_as_the_tag_base() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (uuid, id) = seeded[0].clone();

    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["ov1".into(), "ov2".into()]),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        add_tags: vec!["new".into()],
        remove_tags: vec!["ov1".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &[uuid], &edit, user_id)
        .await
        .unwrap();

    let book = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        book.subjects,
        vec!["ov2", "new"],
        "base must be the prior override subjects, not the scanned list"
    );
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_deletes_tags_orphaned_by_a_remove_delta() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (uuid, _) = seeded[0].clone();

    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["ov1".into(), "ov2".into()]),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        remove_tags: vec!["ov1".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, std::slice::from_ref(&uuid), &edit, user_id)
        .await
        .unwrap();

    assert_eq!(
        tag_row_count(&pool, "ov1").await,
        0,
        "the remove delta dropped ov1's last membership — row reaped"
    );
    assert_eq!(tag_row_count(&pool, "ov2").await, 1);
    // a.epub's canonical tags are shadowed by the override but still
    // linked — scanned truth survives for revert.
    assert_eq!(tag_row_count(&pool, "scifi").await, 1);
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_preserves_prior_override_fields_and_cover_flag() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (uuid, _) = seeded[0].clone();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Prior Title".into()),
            ..Default::default()
        },
        true,
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        publisher: Some("Bulk House".into()),
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, std::slice::from_ref(&uuid), &edit, user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .expect("override row should exist");
    assert_eq!(loaded.title, Some("Prior Title".into()));
    assert_eq!(loaded.publisher, Some("Bulk House".into()));
    assert!(has_cover, "a bulk text edit must not clear the cover flag");
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_leaves_subjects_untouched_when_no_tag_deltas() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (uuid, _) = seeded[0].clone();

    let edit = BulkMetadataEdit {
        publisher: Some("Bulk House".into()),
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, std::slice::from_ref(&uuid), &edit, user_id)
        .await
        .unwrap();

    let (loaded, _) = get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .expect("override row should exist");
    assert_eq!(
        loaded.subjects, None,
        "a scalar-only bulk edit must not freeze the scanned tag list into an override"
    );
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_rolls_back_everything_when_a_uuid_is_unknown() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (good_uuid, _) = seeded[0].clone();

    let edit = BulkMetadataEdit {
        publisher: Some("Bulk House".into()),
        ..Default::default()
    };
    let err = bulk_merge_metadata_overrides(
        &pool,
        &[good_uuid.clone(), "no-such-uuid".into()],
        &edit,
        user_id,
    )
    .await
    .unwrap_err();
    assert!(matches!(err, MetadataOverridesError::BookNotFound(u) if u == "no-such-uuid"));

    assert!(
        get_metadata_overrides(&pool, &good_uuid)
            .await
            .unwrap()
            .is_none(),
        "the known book must not have gained an override row"
    );
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_errors_with_too_many_tags_when_a_book_would_exceed_the_cap()
{
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let (uuid, _) = seeded[0].clone();

    let full: Vec<String> = (0..MetadataOverrides::MAX_SUBJECTS)
        .map(|i| format!("tag{i}"))
        .collect();
    merge_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(full),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        add_tags: vec!["one-too-many".into()],
        ..Default::default()
    };
    let err = bulk_merge_metadata_overrides(&pool, std::slice::from_ref(&uuid), &edit, user_id)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        MetadataOverridesError::TooManyValues { uuid: u, field: "tag", max }
            if u == uuid && max == MetadataOverrides::MAX_SUBJECTS
    ));
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_rebuilds_fts_for_each_book() {
    let _covers = CoversTempDir::new("fts_bulk_override");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let seeded = seed_two_books_for_bulk(&pool).await;
    let uuids: Vec<String> = seeded.iter().map(|(u, _)| u.clone()).collect();

    let edit = BulkMetadataEdit {
        add_tags: vec!["bulktag".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &uuids, &edit, user_id)
        .await
        .unwrap();

    for (_, id) in &seeded {
        let tags: String = sqlx::query_scalar("SELECT tags FROM books_fts WHERE rowid = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert!(
            tags.contains("bulktag"),
            "books_fts.tags for rowid {id} should contain the bulk-added tag, got: {tags}"
        );
    }
}

#[tokio::test]
async fn bulk_merge_metadata_overrides_applies_correctly_across_a_batch_past_the_chunk_boundary() {
    // Both batch fetches chunk at 500 uuids; 520 books exercises the
    // two-chunk path for each, including a boundary-straddling override.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    const BOOK_COUNT: usize = 520;
    let books: Vec<_> = (0..BOOK_COUNT)
        .map(|i| {
            indexed(
                &format!("book-{i:04}.epub"),
                Some(&format!("Title {i}")),
                &["Old Author"],
                &["shared"],
                None,
                None,
            )
        })
        .collect();
    replace_books(&pool, "/lib", books).await.unwrap();
    let mut seeded = list_books(&pool, "/lib").await.unwrap();
    seeded.sort_by(|a, b| a.filename.cmp(&b.filename));
    assert_eq!(seeded.len(), BOOK_COUNT);
    let uuids: Vec<String> = seeded
        .iter()
        .map(|b| b.unique_identifier.clone().unwrap())
        .collect();

    // Give one book past the first 500-uuid chunk boundary a pre-existing
    // override with its own subjects, so the in-tx override-row batch read
    // (not just the pre-tx effective-subjects fetch) is exercised for a
    // second-chunk uuid too.
    let boundary_uuid = uuids[510].clone();
    merge_metadata_overrides(
        &pool,
        &boundary_uuid,
        &MetadataOverrides {
            subjects: Some(vec!["preexisting".into()]),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();

    let edit = BulkMetadataEdit {
        publisher: Some("Batch House".into()),
        add_tags: vec!["bulklabel".into()],
        remove_tags: vec!["shared".into(), "preexisting".into()],
        ..Default::default()
    };
    bulk_merge_metadata_overrides(&pool, &uuids, &edit, user_id)
        .await
        .unwrap();

    let merged = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(merged.len(), BOOK_COUNT);
    for book in &merged {
        assert_eq!(
            book.publisher,
            Some("Batch House".into()),
            "book {:?} missed the bulk publisher edit",
            book.unique_identifier
        );
        assert_eq!(
            book.subjects,
            vec!["bulklabel".to_string()],
            "book {:?} has the wrong effective subjects after the bulk tag delta",
            book.unique_identifier
        );
    }
}
