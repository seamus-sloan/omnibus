//! `sync_books` over a mixed `SyncPlan`: book-id preservation across the
//! Unchanged and Changed buckets, override survival, the Backfill bucket's
//! stat-only write, the New bucket's returning-file pre-fetch, and the
//! series cleanup a Changed scan triggers.

use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

use super::super::*;
use crate::books::{list_books, list_indexed_rows, search_books};
use crate::ebook::IndexedBook;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::test_support::{indexed, indexed_with_stat, CoversTempDir};

/// Seed two books via `replace_books`, then `sync_books` with no
/// diff buckets at all. Both ids must survive — that's the whole
/// point of the refactor.
#[tokio::test]
async fn sync_preserves_book_id_for_unchanged() {
    let _covers = CoversTempDir::new("sync_unchanged");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &[], &[], None, None),
            indexed("b.epub", Some("B"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let before: Vec<_> = list_books(&pool, "/lib")
        .await
        .unwrap()
        .into_iter()
        .map(|b| (b.filename.clone(), b.id))
        .collect();

    sync_books(&pool, "/lib", SyncPlan::default())
        .await
        .unwrap();

    let after: Vec<_> = list_books(&pool, "/lib")
        .await
        .unwrap()
        .into_iter()
        .map(|b| (b.filename.clone(), b.id))
        .collect();
    assert_eq!(before, after, "ids must be preserved across a no-op sync");
}

#[tokio::test]
async fn sync_preserves_book_id_for_changed() {
    let _covers = CoversTempDir::new("sync_changed");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Old Title"),
            &["Old Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

    // One Changed entry — same filename so same uuid, new title + author.
    let plan = SyncPlan {
        changed_books: vec![IndexedBook {
            metadata: EbookMetadata {
                filename: "a.epub".into(),
                title: Some("New Title".into()),
                creators: vec![Contributor {
                    name: "New Author".into(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 999,
            size_bytes: 42,
            word_count: None,
        }],
        ..Default::default()
    };
    sync_books(&pool, "/lib", plan).await.unwrap();

    let after = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].id, original_id, "books.id must be preserved");
    assert_eq!(after[0].title.as_deref(), Some("New Title"));
    assert_eq!(after[0].creators.len(), 1);
    assert_eq!(after[0].creators[0].name, "New Author");
}

/// A user-supplied metadata override (keyed by `book_uuid`, no FK to
/// `books.id`) must still apply after a Changed UPDATE — proving
/// that the in-place UPDATE doesn't accidentally rotate the uuid
/// and that the overrides table isn't touched by sync_books.
#[tokio::test]
async fn sync_overrides_survive_changed() {
    let _covers = CoversTempDir::new("sync_overrides");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("Scanned"), &[], &[], None, None)],
    )
    .await
    .unwrap();
    let book_uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
        .uuid
        .clone();

    // Write a user override that renames the title.
    let overrides = MetadataOverrides {
        title: Some("User Title".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &book_uuid, &overrides, false, user_id)
        .await
        .unwrap();

    // Now Change the book — the scan would happily say "Scanned"
    // again, but the override should still surface "User Title".
    let plan = SyncPlan {
        changed_books: vec![indexed_with_stat("a.epub", Some("Scanned v2"), 100, 100)],
        ..Default::default()
    };
    sync_books(&pool, "/lib", plan).await.unwrap();

    let after = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(after[0].title.as_deref(), Some("User Title"));
}

/// One sync covering all four mutating branches at once. Unchanged
/// ids stay put; Changed id stays put; New gets a fresh id;
/// Removed disappears.
#[tokio::test]
async fn sync_mixed_diff_in_one_transaction() {
    let _covers = CoversTempDir::new("sync_mixed");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("keep.epub", Some("Keep"), &[], &[], None, None),
            indexed("edit.epub", Some("Old Edit"), &[], &[], None, None),
            indexed("gone.epub", Some("Gone"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let before: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
        .await
        .unwrap()
        .into_iter()
        .map(|b| (b.filename.clone(), b.id))
        .collect();
    let gone_uuid = crate::test_support::uuid_by_scan_key(&pool, "gone.epub").await;

    let plan = SyncPlan {
        new_books: vec![indexed_with_stat("add.epub", Some("Added"), 100, 100)],
        changed_books: vec![indexed_with_stat("edit.epub", Some("New Edit"), 200, 200)],
        removed_uuids: vec![gone_uuid],
        ..Default::default()
    };
    sync_books(&pool, "/lib", plan).await.unwrap();

    let after: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
        .await
        .unwrap()
        .into_iter()
        .map(|b| (b.filename.clone(), b.id))
        .collect();

    assert_eq!(after.len(), 3);
    assert_eq!(after.get("keep.epub"), before.get("keep.epub"));
    assert_eq!(after.get("edit.epub"), before.get("edit.epub"));
    assert!(after.contains_key("add.epub"));
    assert!(!after.contains_key("gone.epub"));
}

/// FTS5 row carries `rowid = books.id`. After a Changed UPDATE the
/// rowid must still equal the preserved id, and the index content
/// must reflect the new title.
#[tokio::test]
async fn sync_fts_row_consistent_after_changed() {
    let _covers = CoversTempDir::new("sync_fts");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("Antarctica"), &[], &[], None, None)],
    )
    .await
    .unwrap();
    let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            changed_books: vec![indexed_with_stat("a.epub", Some("Borealis"), 200, 200)],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let stale = search_books(&pool, "/lib", "Antarctica").await.unwrap();
    let fresh = search_books(&pool, "/lib", "Borealis").await.unwrap();
    assert!(stale.is_empty(), "old title must not match after change");
    assert_eq!(fresh.len(), 1);
    assert_eq!(fresh[0].id, original_id, "FTS rowid stable across change");
}

/// The Backfill bucket fills in the post-migration sentinel stat
/// values without touching any metadata columns. Confirm both
/// invariants: stat populated, OPF-derived fields untouched.
#[tokio::test]
async fn sync_backfill_writes_stat_without_touching_metadata() {
    let _covers = CoversTempDir::new("sync_backfill");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("Original"), &[], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
        .uuid
        .clone();
    // Confirm the row started at the (0, 0) sentinel — replace_books
    // wrote the IndexedBook stats (which the test fixture defaults
    // to 0).
    let pre = list_indexed_rows(&pool, "/lib").await.unwrap();
    assert_eq!(pre[0].mtime_epoch, 0);
    assert_eq!(pre[0].size_bytes, 0);

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            backfill: vec![(uuid, 1234, 5678)],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let post = list_indexed_rows(&pool, "/lib").await.unwrap();
    assert_eq!(post[0].mtime_epoch, 1234);
    assert_eq!(post[0].size_bytes, 5678);
    // Title is untouched — backfill must not have triggered any
    // metadata writes.
    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books[0].title.as_deref(), Some("Original"));
}

/// The batched New path pre-fetches a `HashMap` of `(scan_key -> (id, uuid))`
/// before entering the per-book loop. When a mix of returning-file (fileless)
/// entries and truly-new entries lands in one plan, the returning files must
/// re-attach to their existing rows (preserving `books.uuid`) and the new files
/// must mint fresh rows — all resolved off the in-memory map with no per-book
/// SELECT.
#[tokio::test]
async fn sync_new_batch_pre_fetch_re_attaches_returning_and_inserts_fresh() {
    let _covers = CoversTempDir::new("new_batch_prefetch");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Seed 3 books, then mark them all fileless so the next sync sees their
    // scan_keys as candidates for re-attach.
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &[], None, None),
            indexed("b.epub", Some("B"), &["Y"], &[], None, None),
            indexed("c.epub", Some("C"), &["Z"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let uuid_a = crate::test_support::uuid_by_scan_key(&pool, "a.epub").await;
    let uuid_b = crate::test_support::uuid_by_scan_key(&pool, "b.epub").await;
    let uuid_c = crate::test_support::uuid_by_scan_key(&pool, "c.epub").await;
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid_a.clone(), uuid_b.clone(), uuid_c.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // One plan: 2 returning files (a.epub, c.epub) + 1 brand-new (d.epub). The
    // batched scan_key SELECT resolves a.epub + c.epub to their existing rows;
    // d.epub falls through the map lookup, tries the attach heuristic, then
    // inserts.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                indexed("a.epub", Some("A"), &["X"], &[], None, None),
                indexed("c.epub", Some("C"), &["Z"], &[], None, None),
                indexed("d.epub", Some("D"), &["W"], &[], None, None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let visible = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        visible.len(),
        3,
        "two re-attached rows + one new row are visible; b.epub stays fileless"
    );
    assert_eq!(
        crate::test_support::uuid_by_scan_key(&pool, "a.epub").await,
        uuid_a,
        "returning a.epub keeps its durable uuid (re-attached, not re-minted)"
    );
    assert_eq!(
        crate::test_support::uuid_by_scan_key(&pool, "c.epub").await,
        uuid_c,
        "returning c.epub keeps its durable uuid (re-attached, not re-minted)"
    );
    // The fileless row (b.epub) survives with its uuid; the new row (d.epub)
    // has a fresh uuid.
    assert_eq!(
        crate::test_support::uuid_by_scan_key(&pool, "b.epub").await,
        uuid_b,
        "fileless b.epub retains its scan_key + uuid across the sync"
    );
    let uuid_d = crate::test_support::uuid_by_scan_key(&pool, "d.epub").await;
    assert!(![uuid_a, uuid_b, uuid_c].contains(&uuid_d));
}

/// Re-scanning a book whose OPF no longer names its old series leaves that
/// series with zero members, and `sync_books` must delete the now-childless
/// row rather than leave a 0-book series in browse.
#[tokio::test]
async fn sync_books_deletes_a_series_left_with_zero_books_by_a_changed_scan() {
    let _covers = CoversTempDir::new("sync_orphan_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Author A"],
            &[],
            Some(("Old Saga", "1")),
            None,
        )],
    )
    .await
    .unwrap();

    let plan = SyncPlan {
        changed_books: vec![indexed(
            "a.epub",
            Some("A"),
            &["Author B"],
            &[],
            Some(("New Saga", "1")),
            None,
        )],
        ..Default::default()
    };
    sync_books(&pool, "/lib", plan).await.unwrap();

    let series: Vec<String> = sqlx::query_scalar("SELECT name FROM series ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series,
        vec!["New Saga".to_string()],
        "orphan series deleted"
    );

    let authors: Vec<String> = sqlx::query_scalar("SELECT name FROM authors ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        authors,
        vec!["Author B".to_string()],
        "orphan author deleted"
    );
}
