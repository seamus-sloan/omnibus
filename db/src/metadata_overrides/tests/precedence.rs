//! Which metadata source wins: the default override-over-scanned
//! precedence, and what changing a library's configured precedence flips
//! for `get_book` and `list_books`.

use omnibus_shared::{MetadataOverrides, MetadataSource};

use crate::books::{get_book, list_books};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;

// -----------------------------------------------------------------
// F5.1 per-library metadata precedence (#972)
// -----------------------------------------------------------------

/// AC1: with no explicit per-library configuration, a book's override
/// still wins over its scanned metadata — the default precedence order
/// reproduces today's hardcoded "override always wins" behavior.
#[tokio::test]
async fn apply_overrides_wins_by_default_precedence() {
    let _covers = CoversTempDir::new("precedence_default");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "precedence.epub",
            Some("Scanned Title"),
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
    let id = books[0].id;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Overridden Title".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.title.as_deref(), Some("Overridden Title"));
    assert!(merged.has_override);
}

/// AC2: reordering a library's precedence so `embedded_tags` outranks
/// `omnibus_overrides` flips which source wins for a field on the very
/// next read — the override row itself is untouched, only which source
/// the merge picks changes. Covers the single-book read path
/// (`get_book` -> `apply_overrides`).
#[tokio::test]
async fn changing_library_precedence_flips_which_source_wins_for_get_book() {
    let _covers = CoversTempDir::new("precedence_flip_single");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "flip.epub",
            Some("Scanned Title"),
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
    let id = books[0].id;

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Overridden Title".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // Default precedence: override wins.
    assert_eq!(
        get_book(&pool, id).await.unwrap().unwrap().title.as_deref(),
        Some("Overridden Title")
    );

    // Rank embedded_tags above omnibus_overrides for this library.
    crate::settings::set_metadata_precedence(
        &pool,
        "/lib",
        &[
            MetadataSource::FolderStructure,
            MetadataSource::OmnibusOverrides,
            MetadataSource::OpfSidecar,
            MetadataSource::EmbeddedTags,
            MetadataSource::ProviderMatch,
        ],
    )
    .await
    .unwrap();

    // Same override row, different outcome: scanned metadata wins now.
    let after = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(after.title.as_deref(), Some("Scanned Title"));
    assert!(
        !after.has_override,
        "has_override must reflect whether the override is actually visible"
    );
}

/// Same as the single-book test above, but through the bulk list path
/// (`list_books` -> `merge_overrides_into_books`) so the bulk precedence
/// lookup is covered too.
#[tokio::test]
async fn changing_library_precedence_flips_which_source_wins_for_list_books() {
    let _covers = CoversTempDir::new("precedence_flip_bulk");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "flip-bulk.epub",
            Some("Scanned Title"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let uuid = list_books(&pool, "/lib").await.unwrap()[0]
        .unique_identifier
        .clone()
        .unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Overridden Title".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    assert_eq!(
        list_books(&pool, "/lib").await.unwrap()[0].title,
        Some("Overridden Title".to_string())
    );

    crate::settings::set_metadata_precedence(
        &pool,
        "/lib",
        &[
            MetadataSource::FolderStructure,
            MetadataSource::OmnibusOverrides,
            MetadataSource::OpfSidecar,
            MetadataSource::EmbeddedTags,
            MetadataSource::ProviderMatch,
        ],
    )
    .await
    .unwrap();

    assert_eq!(
        list_books(&pool, "/lib").await.unwrap()[0].title,
        Some("Scanned Title".to_string())
    );
}

/// AC4: a book under a library that has never had its precedence touched
/// still merges overrides normally (default order) — unrelated libraries
/// / never-configured precedence rows must not regress existing
/// metadata-edit behavior.
#[tokio::test]
async fn apply_overrides_unaffected_by_precedence_of_a_different_library() {
    let _covers = CoversTempDir::new("precedence_other_lib");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib-a",
        vec![indexed(
            "a.epub",
            Some("Scanned A"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let uuid = list_books(&pool, "/lib-a").await.unwrap()[0]
        .unique_identifier
        .clone()
        .unwrap();
    let id = list_books(&pool, "/lib-a").await.unwrap()[0].id;

    // Flip precedence for an unrelated, never-scanned library path.
    crate::settings::set_metadata_precedence(
        &pool,
        "/lib-b",
        &[
            MetadataSource::FolderStructure,
            MetadataSource::OmnibusOverrides,
            MetadataSource::OpfSidecar,
            MetadataSource::EmbeddedTags,
            MetadataSource::ProviderMatch,
        ],
    )
    .await
    .unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Overridden A".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // /lib-a still uses the default order, so its override still wins.
    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.title.as_deref(), Some("Overridden A"));
}
