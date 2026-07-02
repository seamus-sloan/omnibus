//! Tests for the metadata-overrides write path (upsert/merge/get/delete),
//! its FTS rebuild, and the series-link materialization. Mirrors the
//! pre-split inline `#[cfg(test)] mod tests` block.

use super::*;
use crate::books::{get_book, list_books, search_books};
use crate::palette::search_palette;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};
use omnibus_shared::MetadataOverrides;

// -----------------------------------------------------------------
// F5.1 Metadata overrides
// -----------------------------------------------------------------
#[tokio::test]
async fn upsert_and_get_metadata_overrides_roundtrips() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Create a user for updated_by.
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let ov = MetadataOverrides {
        title: Some("New Title".into()),
        description: Some("A new description".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "test-uuid-1", &ov, false, user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "test-uuid-1")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(loaded.title, Some("New Title".into()));
    assert_eq!(loaded.description, Some("A new description".into()));
    assert_eq!(loaded.publisher, None);
    assert!(!has_cover);
}
#[tokio::test]
async fn merge_metadata_overrides_accumulates_fields_and_preserves_cover_flag() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // Seed an existing override carrying a title AND a user-uploaded cover.
    let initial = MetadataOverrides {
        title: Some("First Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "merge-uuid", &initial, true, user_id)
        .await
        .unwrap();

    // A later edit touching only `description` must not clobber the title
    // (the incremental-edit contract the TOCTOU race nullified) and must
    // not reset the cover flag (the pre-#166 reset bug).
    let edit = MetadataOverrides {
        description: Some("Added later".into()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, "merge-uuid", &edit, user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "merge-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(
        loaded.title,
        Some("First Title".into()),
        "prior title must survive a description-only merge"
    );
    assert_eq!(loaded.description, Some("Added later".into()));
    assert!(
        has_cover,
        "has_cover_override must carry forward across a text-only merge"
    );
}
#[tokio::test]
async fn merge_metadata_overrides_creates_row_when_absent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let edit = MetadataOverrides {
        title: Some("Fresh".into()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, "fresh-uuid", &edit, user_id)
        .await
        .unwrap();
    let (loaded, has_cover) = get_metadata_overrides(&pool, "fresh-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(loaded.title, Some("Fresh".into()));
    assert!(!has_cover, "a brand-new merged row has no cover override");
}
/// Two concurrent saves to the same book (e.g. the edit form open in two
/// tabs, or a network retry firing twice) each touch a different field.
/// Because the rpc/REST save paths route through `merge_metadata_overrides`
/// — whose read-merge-write runs under a single `BEGIN IMMEDIATE` — neither
/// write may be silently dropped: both fields must survive regardless of
/// interleaving. A barrier releases both tasks into the merge at the same
/// instant so the test exercises real contention
/// rather than letting the first save finish before the second starts.
#[tokio::test]
async fn merge_metadata_overrides_concurrent_saves_dont_drop_writes() {
    use std::sync::Arc;
    use tokio::sync::Barrier;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let barrier = Arc::new(Barrier::new(2));
    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let barrier_a = barrier.clone();
    let barrier_b = barrier.clone();
    let save_title = tokio::spawn(async move {
        barrier_a.wait().await;
        merge_metadata_overrides(
            &pool_a,
            "race-uuid",
            &MetadataOverrides {
                title: Some("Title From Tab A".into()),
                ..Default::default()
            },
            user_id,
        )
        .await
    });
    let save_publisher = tokio::spawn(async move {
        barrier_b.wait().await;
        merge_metadata_overrides(
            &pool_b,
            "race-uuid",
            &MetadataOverrides {
                publisher: Some("Publisher From Tab B".into()),
                ..Default::default()
            },
            user_id,
        )
        .await
    });

    save_title.await.unwrap().unwrap();
    save_publisher.await.unwrap().unwrap();

    let (loaded, _) = get_metadata_overrides(&pool, "race-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(
        loaded.title,
        Some("Title From Tab A".into()),
        "tab A's title must not be lost to tab B's concurrent save"
    );
    assert_eq!(
        loaded.publisher,
        Some("Publisher From Tab B".into()),
        "tab B's publisher must not be lost to tab A's concurrent save"
    );
}
#[tokio::test]
async fn get_metadata_overrides_returns_none_when_absent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let result = get_metadata_overrides(&pool, "nonexistent-uuid")
        .await
        .unwrap();
    assert!(result.is_none());
}
#[tokio::test]
async fn delete_metadata_overrides_removes_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ov = MetadataOverrides {
        title: Some("Override".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "del-uuid", &ov, false, user_id)
        .await
        .unwrap();
    assert!(get_metadata_overrides(&pool, "del-uuid")
        .await
        .unwrap()
        .is_some());

    delete_metadata_overrides(&pool, "del-uuid").await.unwrap();
    assert!(get_metadata_overrides(&pool, "del-uuid")
        .await
        .unwrap()
        .is_none());
}
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
async fn delete_overrides_reverts_to_scanned() {
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
/// Verify that `MetadataOverrides::merge` correctly layers a second edit
/// on top of a first without losing the first edit's fields.
#[tokio::test]
async fn merge_preserves_prior_overrides() {
    let first = MetadataOverrides {
        title: Some("Edited Title".into()),
        publisher: Some("Edited Publisher".into()),
        ..Default::default()
    };
    let second = MetadataOverrides {
        description: Some("New description".into()),
        ..Default::default()
    };
    let merged = first.merge(&second);
    // second's description wins
    assert_eq!(merged.description.as_deref(), Some("New description"));
    // first's title and publisher are preserved (not wiped by None)
    assert_eq!(merged.title.as_deref(), Some("Edited Title"));
    assert_eq!(merged.publisher.as_deref(), Some("Edited Publisher"));
    // unset in both stays None
    assert_eq!(merged.language, None);
}
#[tokio::test]
async fn upsert_overrides_materializes_series_link_for_new_series() {
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

/// The override row and the `books_series_link` row must land
/// atomically: after `upsert_metadata_overrides` commits, a direct
/// SELECT on `books_series_link` must already reflect the new series.
/// Guards the post-#576 invariant that the link materialize runs inside
/// the same transaction as the override row, so the book detail page
/// never observes a fresh override against a stale link.
#[tokio::test]
async fn upsert_overrides_persists_books_series_link_row_for_new_series() {
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
