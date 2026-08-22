//! The upsert / merge / get / delete round trip: which fields a later edit
//! preserves, which an empty string clears, the concurrent-save merge, the
//! corrupt-blob error, and the `books.last_modified` bump and export-EPUB
//! cache eviction a write triggers.

use omnibus_shared::MetadataOverrides;

use crate::books::{get_book, get_book_by_uuid, list_books};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir, EnvVarGuard};

use super::super::*;

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

// -----------------------------------------------------------------
// #1658 isbn10 / print_pages override fields
// -----------------------------------------------------------------

#[tokio::test]
async fn upsert_and_get_metadata_overrides_roundtrips_isbn10_and_print_pages() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let ov = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        print_pages: Some(412),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "isbn10-uuid", &ov, false, user_id)
        .await
        .unwrap();

    let (loaded, _) = get_metadata_overrides(&pool, "isbn10-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(loaded.isbn10, Some("0134685997".into()));
    assert_eq!(loaded.print_pages, Some(412));
}

#[tokio::test]
async fn merge_metadata_overrides_preserves_isbn10_and_print_pages_when_a_later_edit_omits_them() {
    // AC2: a client that predates these fields must not clobber them by
    // omission — mirrors `merge_metadata_overrides_accumulates_fields_and_preserves_cover_flag`.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let initial = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        print_pages: Some(412),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "preserve-uuid", &initial, false, user_id)
        .await
        .unwrap();

    let edit = MetadataOverrides {
        description: Some("Added later".into()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, "preserve-uuid", &edit, user_id)
        .await
        .unwrap();

    let (loaded, _) = get_metadata_overrides(&pool, "preserve-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(
        loaded.isbn10,
        Some("0134685997".into()),
        "isbn10 must survive a description-only merge"
    );
    assert_eq!(
        loaded.print_pages,
        Some(412),
        "print_pages must survive a description-only merge"
    );
    assert_eq!(loaded.description, Some("Added later".into()));
}

#[tokio::test]
async fn merge_metadata_overrides_clears_isbn10_with_an_empty_string() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    let initial = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, "clear-isbn10-uuid", &initial, false, user_id)
        .await
        .unwrap();

    let clear = MetadataOverrides {
        isbn10: Some(String::new()),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, "clear-isbn10-uuid", &clear, user_id)
        .await
        .unwrap();

    let (loaded, _) = get_metadata_overrides(&pool, "clear-isbn10-uuid")
        .await
        .unwrap()
        .expect("overrides should exist");
    assert_eq!(loaded.isbn10, Some(String::new()));
}

#[tokio::test]
async fn get_book_returns_effective_isbn10_and_print_pages_from_the_override_layer() {
    // AC1: the round-trip endpoint reads this same merged book back.
    let _covers = CoversTempDir::new("isbn10_print_pages_read");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = crate::test_support::seed_synced_ebook(&pool, "b.epub", "T", "A").await;

    let before = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(before.isbn10, None);
    assert_eq!(before.print_pages, None);

    let ov = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        print_pages: Some(412),
        ..Default::default()
    };
    merge_metadata_overrides(&pool, &uuid, &ov, user_id)
        .await
        .unwrap();

    let after = get_book_by_uuid(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(after.isbn10.as_deref(), Some("0134685997"));
    assert_eq!(after.print_pages, Some(412));
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
