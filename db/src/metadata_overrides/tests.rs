//! Tests for the metadata-overrides write path (upsert/merge/get/delete),
//! its FTS rebuild, and the series-link materialization. Mirrors the
//! pre-split inline `#[cfg(test)] mod tests` block.

use omnibus_shared::{BulkMetadataEdit, Contributor, MetadataOverrides, MetadataSource};

use super::upsert::override_match_keys;
use super::*;
use crate::books::{get_book, list_books, search_books};
use crate::palette::search_palette;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir, EnvVarGuard};

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

/// A grid quick-edit "clear this field" save must land as a real clear,
/// not a silent no-op. `merge_metadata_overrides` reads an incoming
/// `None` as "untouched — keep whatever override already exists" (that's
/// what lets an edit that only touches one field preserve the rest), so
/// a caller that represents "the user cleared series/publisher" as
/// `None` can never actually clear it — the prior override value
/// survives forever. The correct clear payload is `Some("")`:
/// `Option::or` always prefers a `Some`, even an empty one, so the merge
/// overwrites the prior override value. Unlike `isbn13` — which
/// `apply_overrides` special-cases to read back as `None` — series and
/// publisher have no such special-casing, so the field reads back as a
/// literal empty string (`Some("")`), not `None`; that's still what the
/// UI (and the full metadata-edit page's `build_overrides`) treats as
/// "cleared". This is what
/// `frontend::pages::landing::table::cells::field_override` now sends
/// for the grid's quick-edit cells.
#[tokio::test]
async fn merge_metadata_overrides_treats_none_as_untouched_but_empty_string_as_clear() {
    let _covers = CoversTempDir::new("clear_field_1085");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "clear.epub",
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

    let initial = MetadataOverrides {
        series: Some("Foundation".into()),
        publisher: Some("Gnome Press".into()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, &uuid, &initial, user_id)
        .await
        .unwrap();
    let with_overrides = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(with_overrides.series.as_deref(), Some("Foundation"));
    assert_eq!(with_overrides.publisher.as_deref(), Some("Gnome Press"));

    // A `None`-based "clear" (the pre-fix grid payload) is a no-op: the
    // prior override values survive the merge untouched.
    let none_clear = MetadataOverrides::default();
    merge_metadata_overrides(&pool, &uuid, &none_clear, user_id)
        .await
        .unwrap();
    let after_none = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        after_none.series.as_deref(),
        Some("Foundation"),
        "a None-based clear payload must not be able to clear series (#1085)"
    );
    assert_eq!(
        after_none.publisher.as_deref(),
        Some("Gnome Press"),
        "a None-based clear payload must not be able to clear publisher (#1085)"
    );

    // The `Some("")` sentinel actually clears.
    let real_clear = MetadataOverrides {
        series: Some(String::new()),
        publisher: Some(String::new()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, &uuid, &real_clear, user_id)
        .await
        .unwrap();
    let after_clear = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(after_clear.series.as_deref(), Some(""));
    assert_eq!(after_clear.publisher.as_deref(), Some(""));
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
async fn get_metadata_overrides_returns_serialization_error_for_corrupt_blob() {
    // The write path serializes valid JSON, but a row can still hold a corrupt
    // `overrides` blob (a hand-edited DB, or a schema predating a field). When
    // the read decodes it via `serde_json::from_str`, the failure must surface
    // as `MetadataOverridesError::Serialization` — the `#[from] serde_json`
    // variant — not a `Db` error. Insert malformed JSON directly to bypass the
    // serialize-on-write and drive the decode failure.
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO metadata_overrides (book_uuid, overrides) VALUES (?, ?)")
        .bind("corrupt-uuid")
        .bind("{ not valid json")
        .execute(&pool)
        .await
        .unwrap();

    let err = get_metadata_overrides(&pool, "corrupt-uuid")
        .await
        .expect_err("corrupt overrides JSON must not decode");
    assert!(
        matches!(err, MetadataOverridesError::Serialization(_)),
        "got {err:?}"
    );
    assert!(
        err.to_string().starts_with("JSON (de)serialization failed"),
        "got {err}"
    );
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
#[tokio::test]
async fn clear_cover_override_is_noop_when_no_override_row_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // No prior `upsert_metadata_overrides` call for this uuid — must not
    // error or fabricate a row.
    clear_cover_override(&pool, "no-such-uuid", 1)
        .await
        .unwrap();
    assert!(get_metadata_overrides(&pool, "no-such-uuid")
        .await
        .unwrap()
        .is_none());
}
#[tokio::test]
async fn clear_cover_override_is_noop_when_cover_override_not_set() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ov = MetadataOverrides {
        title: Some("Text Only".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "text-only-uuid", &ov, false, user_id)
        .await
        .unwrap();

    clear_cover_override(&pool, "text-only-uuid", user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "text-only-uuid")
        .await
        .unwrap()
        .expect("text override row must survive a no-op cover clear");
    assert_eq!(loaded.title.as_deref(), Some("Text Only"));
    assert!(!has_cover);
}
#[tokio::test]
async fn clear_cover_override_preserves_text_overrides_when_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let ov = MetadataOverrides {
        title: Some("Kept Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "mixed-uuid", &ov, true, user_id)
        .await
        .unwrap();

    clear_cover_override(&pool, "mixed-uuid", user_id)
        .await
        .unwrap();

    let (loaded, has_cover) = get_metadata_overrides(&pool, "mixed-uuid")
        .await
        .unwrap()
        .expect("text override must survive a cover-only clear");
    assert_eq!(loaded.title.as_deref(), Some("Kept Title"));
    assert!(!has_cover, "cover flag must be cleared");
}
#[tokio::test]
async fn clear_cover_override_deletes_row_when_no_overrides_remain() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // A cover-only override: no text fields set.
    upsert_metadata_overrides(
        &pool,
        "cover-only-uuid",
        &MetadataOverrides::default(),
        true,
        user_id,
    )
    .await
    .unwrap();

    clear_cover_override(&pool, "cover-only-uuid", user_id)
        .await
        .unwrap();

    assert!(
        get_metadata_overrides(&pool, "cover-only-uuid")
            .await
            .unwrap()
            .is_none(),
        "an override row with nothing left active must be deleted, not left empty"
    );
}
/// #1395: mirrors `delete_metadata_overrides_removes_stale_export_epub_cache`
/// — clearing a cover-only override deletes the whole row (nothing left
/// active), so the stale export cache must go too.
#[tokio::test]
async fn clear_cover_override_removes_stale_export_epub_cache_when_nothing_remains() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    // A cover-only override: clearing it empties the row entirely.
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    let cache_path = crate::epub_rewrite::export_epub_path(id);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();

    clear_cover_override(&pool, &uuid, user_id).await.unwrap();

    assert!(
        !cache_path.exists(),
        "clearing the last active (cover) override must delete the export cache file"
    );
}

/// Counterpart: a cover clear that leaves a text override in place must NOT
/// delete the export cache, since the book still needs a rewrite (just
/// without the cover swap) — the `last_modified` bump already forces
/// `rewritten_epub_path` to regenerate rather than serve a torn-down file.
#[tokio::test]
async fn clear_cover_override_keeps_export_epub_cache_when_text_overrides_remain() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Kept Title".into()),
            ..Default::default()
        },
        true,
        user_id,
    )
    .await
    .unwrap();

    let cache_path = crate::epub_rewrite::export_epub_path(id);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();

    clear_cover_override(&pool, &uuid, user_id).await.unwrap();

    assert!(
        cache_path.exists(),
        "a text override still active means the export cache isn't dead weight yet"
    );
}

#[tokio::test]
async fn clear_cover_override_returns_serialization_error_for_corrupt_blob() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, has_cover_override) \
         VALUES (?, ?, 1)",
    )
    .bind("corrupt-cover-uuid")
    .bind("{ not valid json")
    .execute(&pool)
    .await
    .unwrap();

    let err = clear_cover_override(&pool, "corrupt-cover-uuid", 1)
        .await
        .expect_err("corrupt overrides JSON must not decode");
    assert!(
        matches!(err, MetadataOverridesError::Serialization(_)),
        "got {err:?}"
    );
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

/// Verify that `MetadataOverrides::merge` correctly layers a second edit
/// on top of a first without losing the first edit's fields.
#[tokio::test]
async fn metadata_overrides_merge_preserves_prior_overrides() {
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

/// Happy path for the filesystem-only cover write helper: the bytes land at
/// `<covers_dir>/override-<uuid>.<ext>` with the extension derived from the
/// declared MIME type.
#[test]
fn write_override_cover_writes_bytes_to_the_expected_path() {
    let covers = CoversTempDir::new("write_cover_ok");

    write_override_cover("happy-uuid", "image/png", b"OVERRIDE-BYTES").unwrap();

    let written = covers.path.join("override-happy-uuid.png");
    assert_eq!(std::fs::read(written).unwrap(), b"OVERRIDE-BYTES");
}

/// `create_dir_all` fails when a regular file already occupies the covers
/// dir path, deterministically forcing the `std::io::Error` branch. The
/// failure must surface as the module's typed `MetadataOverridesError::Io`,
/// not a raw `std::io::Error`.
#[test]
fn write_override_cover_returns_typed_error_when_covers_dir_is_unwritable() {
    let covers = CoversTempDir::new("write_cover_fail");
    std::fs::write(&covers.path, b"not a directory").unwrap();

    let err = write_override_cover("some-uuid", "image/png", b"bytes")
        .expect_err("create_dir_all must fail when the covers dir path is a regular file");
    assert!(matches!(err, MetadataOverridesError::Io(_)), "got {err:?}");
}

/// A non-`NotFound` failure in the stale-extension cleanup loop (here, a
/// directory occupying the path a prior override cover would live at, so
/// `remove_file` fails with `IsADirectory` rather than `NotFound`) must
/// propagate as `MetadataOverridesError::Io` instead of being swallowed —
/// silently leaving the stale entry behind would let `find_override_cover_file`
/// keep probing over it ahead of the freshly written cover.
#[test]
fn write_override_cover_returns_typed_error_when_stale_cleanup_hits_a_directory() {
    let covers = CoversTempDir::new("write_cover_cleanup_fail");
    std::fs::create_dir_all(&covers.path).unwrap();
    // `Jpeg` is first in `PROBE_ORDER`, so this is the first cleanup target.
    std::fs::create_dir(covers.path.join("override-clash-uuid.jpg")).unwrap();

    let err = write_override_cover("clash-uuid", "image/png", b"bytes")
        .expect_err("remove_file on a directory must fail instead of silently leaving it behind");
    assert!(matches!(err, MetadataOverridesError::Io(_)), "got {err:?}");
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
// `override_match_keys` (Physical Check-In's fuzzy-rung match keys)
// -----------------------------------------------------------------

/// A normal override with both a title and a first creator normalizes both
/// sides, mirroring the sync writer's derivation of `books.(title_norm,
/// author_norm)` from scanned metadata.
#[test]
fn override_match_keys_normalizes_title_and_first_creator_when_both_set() {
    let ov = MetadataOverrides {
        title: Some("The Great Gatsby".into()),
        creators: Some(vec![
            Contributor {
                name: "F. Scott Fitzgerald".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            },
            Contributor {
                name: "Someone Else".into(),
                role: Some("edt".into()),
                file_as: None,
                id: None,
            },
        ]),
        ..Default::default()
    };
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm.as_deref(), Some("the great gatsby"));
    assert_eq!(
        author_norm.as_deref(),
        Some("f scott fitzgerald"),
        "only the first creator should feed the author match key"
    );
}

/// `title: Some("")` is the documented "clear" sentinel (mirrors
/// `apply_overrides`'s ISBN handling): it must normalize to `None`, not
/// `Some("")`, so the resolver falls back to the scanned `title_norm`
/// instead of matching against an empty string.
#[test]
fn override_match_keys_normalizes_empty_string_clear_sentinel_to_none() {
    let ov = MetadataOverrides {
        title: Some(String::new()),
        creators: Some(vec![Contributor {
            name: String::new(),
            role: None,
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    };
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm, None);
    assert_eq!(author_norm, None);
}

/// No override set for either field passes through as `(None, None)`,
/// signalling the resolver to fall back entirely to the scanned norms.
#[test]
fn override_match_keys_returns_none_for_both_when_no_override_set() {
    let ov = MetadataOverrides::default();
    let (title_norm, author_norm) = override_match_keys(&ov);
    assert_eq!(title_norm, None);
    assert_eq!(author_norm, None);
}

// -----------------------------------------------------------------
// F5.8 export-with-overrides (#1372): override writes must invalidate the
// cache clock so exports (thumb / KEPUB / rewritten EPUB) rebuild after an edit
// -----------------------------------------------------------------
#[tokio::test]
async fn upsert_metadata_overrides_bumps_book_last_modified() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    // Force an old clock so the post-write bump is unambiguously observable.
    sqlx::query("UPDATE books SET last_modified = 1 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let ov = MetadataOverrides {
        title: Some("Baked".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    let last_modified: i64 = sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        last_modified > 1,
        "override save must bump books.last_modified (cache clock), got {last_modified}"
    );
}

/// #1395: once every override is gone, a previously-cached rewritten export
/// EPUB has nothing left to bake — `delete_metadata_overrides` must remove
/// it eagerly rather than leaving it orphaned on disk until (if ever) the
/// book is exported again.
#[tokio::test]
async fn delete_metadata_overrides_removes_stale_export_epub_cache() {
    let export = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.path().as_os_str()));

    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let id = crate::resolve_book_id_by_uuid(&pool, &uuid)
        .await
        .unwrap()
        .unwrap();

    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Baked".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // Stand in for a cache file a prior export would have written — the
    // rewrite itself is exercised elsewhere (`epub_rewrite::tests`).
    let cache_path = crate::epub_rewrite::export_epub_path(id);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();
    assert!(cache_path.exists());

    delete_metadata_overrides(&pool, &uuid).await.unwrap();

    assert!(
        !cache_path.exists(),
        "clearing the last override must delete the now-stale export cache file"
    );
}

#[tokio::test]
async fn delete_metadata_overrides_bumps_book_last_modified() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;
    let ov = MetadataOverrides {
        title: Some("Baked".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();
    sqlx::query("UPDATE books SET last_modified = 1 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    delete_metadata_overrides(&pool, &uuid).await.unwrap();

    let last_modified: i64 = sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        last_modified > 1,
        "reverting overrides must bump last_modified so exports drop back to source, got {last_modified}"
    );
}

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
        MetadataOverridesError::TooManyTags { uuid: u, max } if u == uuid && max == MetadataOverrides::MAX_SUBJECTS
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
