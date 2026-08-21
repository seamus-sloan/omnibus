//! The FTS rebuild an override write triggers: the title, palette, and tags
//! columns it rewrites, what a delete restores, and the batch rebuild across
//! several uuids.

use omnibus_shared::MetadataOverrides;

use crate::books::{get_book, list_books, search_books};
use crate::palette::search_palette;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

use super::super::*;

/// Bug #1: saving a title override must rebuild `books_fts` so search
/// finds the new title and stops matching the original one.
#[tokio::test]
async fn upsert_metadata_overrides_rebuilds_fts_for_title() {
    let _covers = CoversTempDir::new("fts_override_title");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Original Title"),
            &["Author A"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    // Sanity: search finds the original title.
    let hits = search_books(&pool, "/lib", "Original").await.unwrap();
    assert_eq!(hits.len(), 1);

    // Save an override that changes the title.
    let uuid = list_books(&pool, "/lib").await.unwrap()[0]
        .unique_identifier
        .clone()
        .unwrap();
    let ov = MetadataOverrides {
        title: Some("Brand New Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    // Search now matches the overridden title and no longer the original.
    let new_hits = search_books(&pool, "/lib", "Brand").await.unwrap();
    assert_eq!(new_hits.len(), 1);
    assert_eq!(new_hits[0].title.as_deref(), Some("Brand New Title"));
    let old_hits = search_books(&pool, "/lib", "Original").await.unwrap();
    assert!(
        old_hits.is_empty(),
        "FTS still matches the pre-override title"
    );
}
/// Bug #1: the palette uses the same `books_fts` table, so the override
/// rebuild must also surface there.
#[tokio::test]
async fn upsert_metadata_overrides_rebuilds_fts_for_palette() {
    let _covers = CoversTempDir::new("fts_override_palette");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "p.epub",
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
    let ov = MetadataOverrides {
        title: Some("Edited Palette Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
    assert_eq!(palette.books.len(), 1);
}
/// The single-book rebuild overlays *every* override-driven FTS column,
/// not just the title. A tag (`subjects`) override must land in the
/// `books_fts.tags` column via `overlay_overrides` — which now runs in the
/// same transaction as the canonical `upsert_fts` write. Asserting the
/// stored `books_fts.tags` value directly (rather than via `search_books`,
/// which deliberately filters the `tags` column out of its MATCH) guards
/// that the overlay half of the transactional rebuild committed: a partial
/// rebuild that persisted only the canonical `upsert_fts` row would leave
/// the scanned tag here instead of the overridden one.
#[tokio::test]
async fn upsert_metadata_overrides_rebuilds_fts_tags_column() {
    let _covers = CoversTempDir::new("fts_override_tag");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "t.epub",
            Some("A Book"),
            &["Author"],
            &["scannedtag"],
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
            subjects: Some(vec!["overriddentag".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let tags: String = sqlx::query_scalar("SELECT tags FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        tags, "overriddentag",
        "overlay_overrides must have committed the overridden tag into books_fts.tags"
    );
}
/// Bug #1 follow-on: deleting the override should restore the FTS row
/// to the canonical scanned values.
#[tokio::test]
async fn delete_metadata_overrides_restores_fts() {
    let _covers = CoversTempDir::new("fts_override_revert");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "r.epub",
            Some("Canonical Title"),
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
            title: Some("Temporary Override".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    assert_eq!(
        search_books(&pool, "/lib", "Temporary")
            .await
            .unwrap()
            .len(),
        1
    );

    delete_metadata_overrides(&pool, &uuid).await.unwrap();

    // FTS is back to the canonical title; the override token no longer
    // matches.
    assert_eq!(
        search_books(&pool, "/lib", "Canonical")
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(search_books(&pool, "/lib", "Temporary")
        .await
        .unwrap()
        .is_empty());
}
#[tokio::test]
async fn delete_metadata_overrides_reverts_to_scanned() {
    let _covers = CoversTempDir::new("revert");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "revert.epub",
            Some("Original"),
            &["Author"],
            &["fiction"],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    let id = books[0].id;

    // Override.
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Changed".into()),
            subjects: Some(vec!["sci-fi".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(merged.title.as_deref(), Some("Changed"));

    // Delete overrides — should revert to scanned.
    delete_metadata_overrides(&pool, &uuid).await.unwrap();
    let reverted = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(reverted.title.as_deref(), Some("Original"));
    assert_eq!(reverted.subjects, vec!["fiction"]);
    assert!(!reverted.has_override);
}

/// The batch FTS rebuild resolves and rewrites more than one uuid in a
/// single call — exercising the bulk uuid→id resolve + `IN (…)` book fetch
/// that replaced the former two-query-per-uuid loop. Overrides written
/// straight to the table leave `books_fts` stale (no per-save rebuild), so
/// only the batch rebuild can make search reflect both overridden titles.
#[tokio::test]
async fn rebuild_fts_for_books_batch_rewrites_multiple_uuids() {
    let _covers = CoversTempDir::new("fts_batch_multi");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "one.epub",
                Some("AlphaScanned"),
                &["Author A"],
                &[],
                None,
                None,
            ),
            indexed(
                "two.epub",
                Some("BetaScanned"),
                &["Author B"],
                &[],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid_of = |title: &str| {
        books
            .iter()
            .find(|b| b.title.as_deref() == Some(title))
            .and_then(|b| b.unique_identifier.clone())
            .expect("seeded book should exist")
    };
    let uuid_a = uuid_of("AlphaScanned");
    let uuid_b = uuid_of("BetaScanned");

    // Write overrides straight to the table so `books_fts` stays stale; this
    // isolates the batch rebuild's bulk resolve from the per-save rebuild
    // that `upsert_metadata_overrides` would otherwise trigger.
    for (uuid, title) in [(&uuid_a, "AlphaRenamed"), (&uuid_b, "BetaRenamed")] {
        let json = serde_json::to_string(&MetadataOverrides {
            title: Some(title.to_string()),
            ..Default::default()
        })
        .unwrap();
        sqlx::query("INSERT INTO metadata_overrides (book_uuid, overrides) VALUES (?, ?)")
            .bind(uuid)
            .bind(&json)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Sanity: FTS still matches the scanned titles (no rebuild yet).
    assert!(search_books(&pool, "/lib", "AlphaRenamed")
        .await
        .unwrap()
        .is_empty());

    rebuild_fts_for_books_batch(&pool, &[uuid_a, uuid_b])
        .await
        .unwrap();

    // Both overridden titles are now searchable; neither scanned title is.
    assert_eq!(
        search_books(&pool, "/lib", "AlphaRenamed").await.unwrap()[0]
            .title
            .as_deref(),
        Some("AlphaRenamed")
    );
    assert_eq!(
        search_books(&pool, "/lib", "BetaRenamed").await.unwrap()[0]
            .title
            .as_deref(),
        Some("BetaRenamed")
    );
    assert!(search_books(&pool, "/lib", "AlphaScanned")
        .await
        .unwrap()
        .is_empty());
    assert!(search_books(&pool, "/lib", "BetaScanned")
        .await
        .unwrap()
        .is_empty());
}
