use super::*;
use crate::books::{get_book, list_books, list_indexed_rows, search_books};
use crate::covers::get_cover;
use crate::ebook::IndexedBook;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::settings::last_indexed_at;
use crate::test_support::{indexed, indexed_audiobook, indexed_with_stat, CoversTempDir};
use omnibus_shared::{Contributor, EbookMetadata, Identifier, MetadataOverrides};

#[tokio::test]
async fn replace_books_inserts_metadata_and_covers() {
    let _covers = CoversTempDir::new("insert");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                Some(("Saga", "1")),
                Some(("image/jpeg", b"BYTES")),
            ),
            indexed("b.epub", Some("B"), &["Author B"], &[], None, None),
        ],
    )
    .await
    .expect("replace should succeed");

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 2);

    let a = books
        .iter()
        .find(|b| b.title.as_deref() == Some("A"))
        .unwrap();
    let b = books
        .iter()
        .find(|b| b.title.as_deref() == Some("B"))
        .unwrap();

    assert_eq!(a.filename, "a.epub");
    assert_eq!(b.filename, "b.epub");
    assert_eq!(a.creators.len(), 1);
    assert_eq!(a.creators[0].name, "Author A");
    assert_eq!(a.subjects, vec!["fiction".to_string()]);
    assert_eq!(a.series.as_deref(), Some("Saga"));
    assert_eq!(a.series_index.as_deref(), Some("1"));

    let a_uuid = a.unique_identifier.clone().unwrap();
    assert_eq!(
        a.cover_url.as_deref(),
        Some(format!("/api/covers/{a_uuid}").as_str())
    );
    assert_eq!(b.cover_url, None);

    // F1.3: list_books exposes the row insertion timestamp so the
    // landing page can offer a "Newest Added" sort. The migration
    // defaults `books.timestamp` to `datetime('now')`
    // (`YYYY-MM-DD HH:MM:SS`, UTC), so every row surfaces a non-empty
    // sortable string.
    for book in &books {
        let added = book.added_at.as_deref().unwrap_or("");
        assert!(
            !added.is_empty(),
            "added_at should be populated for {:?}",
            book.title
        );
    }

    let cover = get_cover(&pool, a.id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"BYTES".to_vec())));
    assert!(get_cover(&pool, b.id).await.unwrap().is_none());

    assert!(last_indexed_at(&pool, "/lib").await.unwrap().is_some());
}
/// Atrium accent round-trip. `replace_books` writes `metadata.accent`
/// into `books.accent_color`; `list_books` / `get_book` / `search_books`
/// read it back into `EbookMetadata.accent`. Verify the column survives
/// the trip and `None` stays `None` (not coerced to an empty string).
#[tokio::test]
async fn replace_books_round_trips_accent_color() {
    let _covers = CoversTempDir::new("accent_round_trip");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let with_accent = IndexedBook {
        metadata: EbookMetadata {
            filename: "with-accent.epub".into(),
            title: Some("Piranesi".into()),
            creators: vec![Contributor {
                name: "Susanna Clarke".into(),
                ..Default::default()
            }],
            accent: Some("oklch(0.660 0.130 245.0)".into()),
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
    };
    let no_accent = IndexedBook {
        metadata: EbookMetadata {
            filename: "no-accent.epub".into(),
            title: Some("Plain".into()),
            creators: vec![Contributor {
                name: "Anon".into(),
                ..Default::default()
            }],
            accent: None,
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
    };
    replace_books(&pool, "/lib", vec![with_accent, no_accent])
        .await
        .expect("replace should succeed");

    // list_books returns the accent column for every row.
    let books = list_books(&pool, "/lib").await.unwrap();
    let p = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Piranesi"))
        .unwrap();
    let plain = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Plain"))
        .unwrap();
    assert_eq!(p.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
    assert_eq!(plain.accent, None);

    // get_book returns the same value through the single-row path.
    let detail = get_book(&pool, p.id).await.unwrap().unwrap();
    assert_eq!(detail.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
    let detail_plain = get_book(&pool, plain.id).await.unwrap().unwrap();
    assert_eq!(detail_plain.accent, None);
}
/// The write-boundary gate must accept the exact `oklch(L C H)` shape
/// the indexer emits, and reject anything else — including raw hex, CSS
/// keywords, and injection payloads that try to break out of
/// the `style="background: {bg}"` attribute used by Atrium consumers.
///
/// End-to-end gate: writing an `IndexedBook` whose `accent` carries an
/// injection payload must result in `accent_color = NULL` in the DB,
/// not the unsanitized string.
#[tokio::test]
async fn replace_books_drops_unsafe_accent_color() {
    let _covers = CoversTempDir::new("accent_unsafe");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let unsafe_book = IndexedBook {
        metadata: EbookMetadata {
            filename: "shady.epub".into(),
            title: Some("Shady".into()),
            creators: vec![Contributor {
                name: "Anon".into(),
                ..Default::default()
            }],
            accent: Some("red; background: url(x)".into()),
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
    };
    replace_books(&pool, "/lib", vec![unsafe_book])
        .await
        .expect("replace should succeed");
    let books = list_books(&pool, "/lib").await.unwrap();
    let shady = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Shady"))
        .unwrap();
    assert_eq!(shady.accent, None);
}
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
/// Removing a file (F2) drops only the book's `book_files` row; the `books`
/// row, its taxonomy/author links, FTS row, and soft-ref user data are all
/// retained, so the book stays in browse/search — only the grid/facets hide it
/// via their own `EXISTS book_files` filter.
#[tokio::test]
async fn removing_a_books_file_keeps_its_links_and_fts() {
    let _covers = CoversTempDir::new("sync_removed_cascade");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "doomed.epub",
            Some("Doomed"),
            &["Anon"],
            &["fic"],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let book_id = list_books(&pool, "/lib").await.unwrap()[0].id;
    let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
        .uuid
        .clone();

    let plan = SyncPlan {
        removed_uuids: vec![uuid],
        ..Default::default()
    };
    sync_books(&pool, "/lib", plan).await.unwrap();

    let books_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let files_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let link_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE book = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(books_count, 1, "books row retained as a fileless book");
    assert_eq!(files_count, 0, "only the file rows are dropped");
    assert_eq!(link_count, 1, "author link retained");
    assert_eq!(
        fts_count, 1,
        "FTS row retained — fileless books stay searchable"
    );
}
/// F2 acceptance: removing a file makes its book fileless (hidden from the grid but
/// the row + durable `books.uuid` survive); when the same file returns it
/// re-attaches to that row, preserving the uuid (auto-relink). This is what
/// makes user data keyed on `books.uuid` durable across a removed→re-added
/// cycle.
#[tokio::test]
async fn removed_file_goes_fileless_then_returning_file_relinks_same_uuid() {
    let _covers = CoversTempDir::new("fileless_relink");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid1 = crate::test_support::uuid_by_scan_key(&pool, "a.epub").await;

    // File gone → fileless: hidden from the list, but the row + uuid survive.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid1.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "fileless book is hidden from the library grid"
    );
    assert_eq!(
        crate::test_support::uuid_by_scan_key(&pool, "a.epub").await,
        uuid1,
        "fileless book retains its scan_key and durable uuid"
    );

    // File returns → re-attaches to the same row (same uuid), listed again.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let after = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].unique_identifier.as_deref(),
        Some(uuid1.as_str()),
        "returning file relinks to the same uuid (no orphaned user data)"
    );
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
        backfill: vec![],
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
/// Removed books should lose their cover files; survivors' covers
/// must stay intact. Catches "delete every cover on every sync"
/// regressions if anyone ever short-circuits the bucket logic.
#[tokio::test]
async fn sync_cover_sidecar_lifecycle_on_remove() {
    let covers = CoversTempDir::new("sync_cover_remove");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "keep.epub",
                Some("Keep"),
                &[],
                &[],
                None,
                Some(("image/jpeg", b"KEEP_BYTES")),
            ),
            indexed(
                "gone.epub",
                Some("Gone"),
                &[],
                &[],
                None,
                Some(("image/jpeg", b"GONE_BYTES")),
            ),
        ],
    )
    .await
    .unwrap();
    let keep_uuid = crate::test_support::uuid_by_scan_key(&pool, "keep.epub").await;
    let gone_uuid = crate::test_support::uuid_by_scan_key(&pool, "gone.epub").await;
    let keep_path = covers.path.join(format!("{keep_uuid}.jpg"));
    let gone_path = covers.path.join(format!("{gone_uuid}.jpg"));
    assert!(keep_path.exists(), "cover for keep should exist");
    assert!(gone_path.exists(), "cover for gone should exist");

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![gone_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(keep_path.exists(), "survivor cover must remain");
    assert!(!gone_path.exists(), "removed cover must be deleted");
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
/// Empty disk → diff says "remove all" → sync_books wipes the
/// library cleanly. Stress test for the Removed branch.
#[tokio::test]
async fn sync_empty_plan_with_full_removed_clears_library() {
    let _covers = CoversTempDir::new("sync_empty");
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
    let all_uuids: Vec<String> = list_indexed_rows(&pool, "/lib")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.uuid)
        .collect();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: all_uuids,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert!(list_books(&pool, "/lib").await.unwrap().is_empty());
}

/// A single Removed bucket exceeding SQLite's 999-bind cap must succeed after
/// batching: `sync_removed` chunks the id-resolution SELECT *and* the batched
/// DELETE + UPDATE that replaced the per-book `mark_book_files_missing` fan-out.
/// 1000 uuids exercises both the chunk boundary (500 + 500) and the batched-DML
/// path so a regression back to the per-book loop would still pass but the
/// bind-cap failure below would surface immediately.
#[tokio::test]
async fn sync_books_with_removed_above_bind_cap_succeeds() {
    let _covers = CoversTempDir::new("book_remove_chunk");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // 1000 books forces the chunked path through two chunks (500 + 500) and
    // pushes an un-chunked `IN (?, ?, ...)` past the 999-bind cap.
    const N: usize = 1000;
    let new_books: Vec<_> = (0..N)
        .map(|i| indexed(&format!("book{i:04}.epub"), Some("t"), &[], &[], None, None))
        .collect();
    replace_books(&pool, "/lib", new_books).await.unwrap();
    assert_eq!(list_books(&pool, "/lib").await.unwrap().len(), N);

    let all_uuids: Vec<String> = sqlx::query_scalar("SELECT uuid FROM books")
        .fetch_all(&pool)
        .await
        .unwrap();

    // Wholesale remove all 1000 in a single plan.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: all_uuids,
            ..Default::default()
        },
    )
    .await
    .expect("wholesale removal of >500 books must not exceed bind cap");

    // Every row is retained as fileless (books row + FTS survive), grid hides
    // them.
    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "every book is hidden from the grid (fileless) after wholesale removal"
    );
    let books_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let files_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files")
        .fetch_one(&pool)
        .await
        .unwrap();
    let flagged: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE is_missing_files = 1")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(books_total, N as i64, "books rows retained as fileless");
    assert_eq!(files_total, 0, "every book_files row was dropped");
    assert_eq!(
        flagged, N as i64,
        "every row was flagged missing by the batched UPDATE"
    );
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

#[tokio::test]
async fn reindex_replaces_library_atomically() {
    let _covers = CoversTempDir::new("atomic");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Author A"],
            &["fiction"],
            None,
            Some(("image/jpeg", b"OLD")),
        )],
    )
    .await
    .unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Author A"],
            &["fiction"],
            None,
            Some(("image/jpeg", b"NEW")),
        )],
    )
    .await
    .unwrap();

    // No orphan rows in any link table for books that no longer exist.
    for table in [
        "books_authors_link",
        "books_tags_link",
        "books_series_link",
        "books_publishers_link",
        "books_languages_link",
    ] {
        let orphan: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {table} WHERE book NOT IN (SELECT id FROM books)"
        ))
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphan, 0, "{table} should have no orphans");
    }
    let orphan_files: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_files WHERE book_id NOT IN (SELECT id FROM books)",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(orphan_files, 0);

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"NEW".to_vec())));
}
#[tokio::test]
async fn author_dedupes_across_books_case_insensitive() {
    let _covers = CoversTempDir::new("dedupe");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
            indexed("b.epub", Some("B"), &["tolkien"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 1, "NOCASE unique should collapse Tolkien/tolkien");
}
#[tokio::test]
async fn series_index_sorts_numerically() {
    // Regression guard against reintroducing Calibre's TEXT series_index:
    // 10 must sort after 2, not before.
    let _covers = CoversTempDir::new("series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("b.epub", Some("B"), &["A"], &[], Some(("S", "10")), None),
            indexed("a.epub", Some("A"), &["A"], &[], Some(("S", "2")), None),
        ],
    )
    .await
    .unwrap();
    let indices: Vec<f64> =
        sqlx::query_scalar("SELECT series_index FROM books ORDER BY series_index")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(indices, vec![2.0, 10.0]);
}
#[tokio::test]
async fn rename_author_updates_fts_via_trigger() {
    let _covers = CoversTempDir::new("fts_rename");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("Book"),
            &["OldName"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    assert_eq!(
        search_books(&pool, "/lib", "OldName").await.unwrap().len(),
        1
    );

    sqlx::query("UPDATE authors SET name = 'NewName' WHERE name = 'OldName'")
        .execute(&pool)
        .await
        .unwrap();

    assert_eq!(
        search_books(&pool, "/lib", "NewName").await.unwrap().len(),
        1
    );
    assert_eq!(
        search_books(&pool, "/lib", "OldName").await.unwrap().len(),
        0
    );
}
#[tokio::test]
async fn reindex_keeps_fts_row_count_in_sync() {
    let _covers = CoversTempDir::new("fts_reindex");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["X"], &[], None, None),
            indexed("b.epub", Some("B"), &["Y"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    // Reindex with one fewer book.
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    // F2: removed book `b` is retained as a fileless book and keeps its FTS
    // row, so the FTS count tracks total books (2), not just file-backed (1).
    let books_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(books_total, 2);
    assert_eq!(
        fts_count, 2,
        "every book — fileless included — keeps an FTS row"
    );
}
#[tokio::test]
async fn overrides_survive_reindex() {
    let _covers = CoversTempDir::new("reindex_survive");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    // First index.
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "survive.epub",
            Some("Original"),
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

    // Save overrides.
    let ov = MetadataOverrides {
        title: Some("Overridden".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();

    // Reindex — replace_books deletes and re-inserts.
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "survive.epub",
            Some("Original"),
            &["Author"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    // The new book row has a new id but the same UUID.
    let books = list_books(&pool, "/lib").await.unwrap();
    let id = books[0].id;
    let merged = get_book(&pool, id).await.unwrap().unwrap();
    assert_eq!(
        merged.title.as_deref(),
        Some("Overridden"),
        "overrides should survive the DELETE/INSERT reindex"
    );
    assert!(merged.has_override);
}
#[tokio::test]
async fn reindex_path_skips_blocked_contributor_and_keeps_real_author() {
    // End-to-end: simulate the reindex path by going through
    // `replace_books` (same insert_metadata_links pipeline). The
    // blocklist guard must keep the junk row from being re-created
    // while the legitimate author is still linked to the book.
    let _covers = CoversTempDir::new("ignored_authors_reindex");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
        .bind("calibre (8.0.0) [https://calibre-ebook.com]")
        .execute(&pool)
        .await
        .unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "the-real-book.epub",
            Some("The Real Book"),
            &["Real Author", "calibre (8.0.0) [https://calibre-ebook.com]"],
            &[],
            None,
            None,
        )],
    )
    .await
    .expect("reindex path should succeed even with a blocked contributor");

    let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
        .bind("calibre (8.0.0) [https://calibre-ebook.com]")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(junk_count, 0, "junk author must not be re-created");

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let book = &books[0];
    let names: Vec<&str> = book.creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Real Author"],
        "only the un-blocked creator should remain linked"
    );
}

#[tokio::test]
async fn reindex_blocklist_matches_case_insensitively() {
    // The batched blocklist join leans on `ignored_authors.name`'s NOCASE
    // collation: a contributor whose casing differs from the stored entry
    // must still be skipped. Guards `insert_author_links` against a
    // regression (e.g. swapping the join operands) that would silently
    // break case-insensitive matching.
    let _covers = CoversTempDir::new("ignored_authors_nocase");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
        .bind("Calibre Bot")
        .execute(&pool)
        .await
        .unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "the-real-book.epub",
            Some("The Real Book"),
            &["Real Author", "calibre bot"],
            &[],
            None,
            None,
        )],
    )
    .await
    .expect("reindex should succeed with a mixed-case blocked contributor");

    let junk_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ? COLLATE NOCASE")
            .bind("calibre bot")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        junk_count, 0,
        "a case-variant blocked author must not be created"
    );

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let names: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["Real Author"],
        "the mixed-case blocked contributor must be skipped"
    );
}

#[tokio::test]
async fn sync_audiobooks_round_trips_accent_color() {
    let _covers = CoversTempDir::new("ab_accent_round_trip");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut with_accent = indexed_audiobook("Author/Book.m4b", "Accented Book", Some("Author"));
    with_accent.accent = Some("oklch(0.660 0.130 245.0)".into());

    let mut no_accent = indexed_audiobook("Author/Plain.m4b", "Plain Book", Some("Author"));
    no_accent.accent = None;

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![with_accent, no_accent],
            ..Default::default()
        },
    )
    .await
    .expect("sync should succeed");

    let books = list_books(&pool, "/lib").await.unwrap();
    let accented = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Accented Book"))
        .unwrap();
    let plain = books
        .iter()
        .find(|b| b.title.as_deref() == Some("Plain Book"))
        .unwrap();
    assert_eq!(accented.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
    assert_eq!(plain.accent, None);

    let detail = get_book(&pool, accented.id).await.unwrap().unwrap();
    assert_eq!(detail.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
}

#[tokio::test]
async fn sync_audiobooks_updates_accent_color_on_changed() {
    let _covers = CoversTempDir::new("ab_accent_update");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let book = indexed_audiobook("Author/Book.m4b", "Book", Some("Author"));
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books[0].accent, None, "initially no accent");

    let mut updated = indexed_audiobook("Author/Book.m4b", "Book", Some("Author"));
    updated.accent = Some("oklch(0.700 0.100 180.0)".into());

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            changed_books: vec![updated],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        books[0].accent.as_deref(),
        Some("oklch(0.700 0.100 180.0)"),
        "accent should be set after update"
    );
}

#[tokio::test]
async fn sync_audiobooks_drops_unsafe_accent_color() {
    let _covers = CoversTempDir::new("ab_accent_unsafe");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Shady.m4b", "Shady", Some("Author"));
    book.accent = Some("red; background: url(x)".into());

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(
        books[0].accent, None,
        "unsafe accent must be sanitized to NULL"
    );
}

// ── Progress-callback contract ────────────────────────────────────
//
// `sync_books_with_progress` and `sync_audiobooks_with_progress` feed
// the worker's `report_progress` so the UI indicator can render a
// determinate `processed / total` bar. The contract: emit `(0, total)`
// before any per-book write, then tick `processed` monotonically up to
// `total` (one tick per New + Changed book), with `total` constant
// across the run. Removed/Backfill counts are deliberately excluded
// from `total` — they're batched and invisible to the user-facing
// "Scanning books… N / M" step.

#[tokio::test]
async fn sync_books_with_progress_emits_initial_zero_and_monotonic_ticks() {
    let _covers = CoversTempDir::new("progress_books");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan = SyncPlan {
        new_books: vec![
            indexed_with_stat("a.epub", Some("A"), 1, 1),
            indexed_with_stat("b.epub", Some("B"), 2, 2),
            indexed_with_stat("c.epub", Some("C"), 3, 3),
        ],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", plan, move |p, t| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    // First tick is (0, total) so the UI can flip from indeterminate
    // spinner to a determinate bar before the first per-book write.
    assert_eq!(ticks.first(), Some(&(0u32, 3u32)));
    // Last tick is (total, total) — we processed every book.
    assert_eq!(ticks.last(), Some(&(3u32, 3u32)));
    // Monotonic non-decreasing processed counter; constant total.
    for pair in ticks.windows(2) {
        assert!(
            pair[0].0 <= pair[1].0,
            "processed must not regress: {ticks:?}"
        );
        assert_eq!(pair[0].1, pair[1].1, "total must stay constant: {ticks:?}");
    }
}

#[tokio::test]
async fn sync_books_with_progress_reports_zero_total_for_no_op_plan() {
    let _covers = CoversTempDir::new("progress_books_noop");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", SyncPlan::default(), move |p, t| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();
    // Even when there's nothing to do we still emit the initial (0, 0)
    // tick so the UI's "switch to determinate bar" code path runs
    // consistently.
    let ticks = ticks.lock().unwrap().clone();
    assert_eq!(ticks, vec![(0u32, 0u32)]);
}

#[tokio::test]
async fn sync_books_with_progress_excludes_removed_and_backfill_from_total() {
    // Total counts the buckets that loop per-book (Changed + New). The
    // Removed and Backfill phases are batched SQL — reporting them as
    // per-book ticks would either inflate `total` for work the user
    // can't see, or under-count `processed` mid-run.
    let _covers = CoversTempDir::new("progress_books_buckets");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("gone.epub", Some("Gone"), &[], &[], None, None),
            indexed("survivor.epub", Some("Survivor"), &[], &[], None, None),
        ],
    )
    .await
    .unwrap();
    let survivor_uuid = crate::test_support::uuid_by_scan_key(&pool, "survivor.epub").await;
    let gone_uuid = crate::test_support::uuid_by_scan_key(&pool, "gone.epub").await;

    let plan = SyncPlan {
        new_books: vec![indexed_with_stat("new.epub", Some("New"), 10, 10)],
        removed_uuids: vec![gone_uuid],
        backfill: vec![(survivor_uuid, 42, 42)],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_books_with_progress(&pool, "/lib", plan, move |p, t| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    // 1 New, 0 Changed → total = 1; Removed + Backfill don't bump it.
    let totals: std::collections::BTreeSet<u32> = ticks.iter().map(|(_, t)| *t).collect();
    assert_eq!(totals.into_iter().collect::<Vec<_>>(), vec![1u32]);
    assert_eq!(ticks.last(), Some(&(1u32, 1u32)));
}

#[tokio::test]
async fn sync_audiobooks_with_progress_emits_initial_zero_and_monotonic_ticks() {
    let _covers = CoversTempDir::new("progress_audiobooks");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan = AudiobookSyncPlan {
        new_books: vec![
            indexed_audiobook("Author/A.m4b", "A", Some("Author")),
            indexed_audiobook("Author/B.m4b", "B", Some("Author")),
        ],
        ..Default::default()
    };
    let ticks = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(u32, u32)>::new()));
    let ticks_cb = ticks.clone();
    sync_audiobooks_with_progress(&pool, "/lib", plan, move |p, t| {
        ticks_cb.lock().unwrap().push((p, t));
    })
    .await
    .unwrap();

    let ticks = ticks.lock().unwrap().clone();
    assert_eq!(ticks.first(), Some(&(0u32, 2u32)));
    assert_eq!(ticks.last(), Some(&(2u32, 2u32)));
    for pair in ticks.windows(2) {
        assert!(
            pair[0].0 <= pair[1].0,
            "processed must not regress: {ticks:?}"
        );
        assert_eq!(pair[0].1, pair[1].1, "total must stay constant: {ticks:?}");
    }
}

/// Build a one-book index entry whose OPF carries the given identifiers.
fn book_with_identifiers(filename: &str, idents: Vec<Identifier>) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some("Two ISBNs".into()),
            creators: vec![Contributor {
                name: "Author".into(),
                ..Default::default()
            }],
            identifiers: idents,
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
    }
}

/// Count `book_identifiers` rows for one scheme on a given book id.
async fn isbn_row_count(pool: &sqlx::SqlitePool, book_id: i64) -> i64 {
    sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_identifiers WHERE book_id = ? AND scheme = 'ISBN'",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// F7: a book whose OPF lists a print ISBN-10 and an ebook ISBN-13 under the
/// same `ISBN` scheme must keep BOTH `book_identifiers` rows. Under the old
/// `(book_id, scheme)` PK + `INSERT OR REPLACE`, the second value clobbered
/// the first; the wider `(book_id, scheme, value)` PK + `INSERT OR IGNORE`
/// keeps both, and the read projection surfaces both.
#[tokio::test]
async fn book_with_two_isbns_keeps_both_identifier_rows() {
    let _covers = CoversTempDir::new("two_isbns");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![book_with_identifiers(
            "two.epub",
            vec![
                Identifier {
                    value: "0000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    let id = books[0].id;

    // Both rows survive at the table level.
    assert_eq!(
        isbn_row_count(&pool, id).await,
        2,
        "both distinct ISBN values must persist in book_identifiers"
    );

    // And both surface through the read projection (list_books + get_book).
    let mut list_isbns: Vec<&str> = books[0]
        .identifiers
        .iter()
        .filter(|i| i.scheme.as_deref() == Some("ISBN"))
        .map(|i| i.value.as_str())
        .collect();
    list_isbns.sort_unstable();
    assert_eq!(list_isbns, vec!["0000000000", "9780000000000"]);

    let detail = get_book(&pool, id).await.unwrap().unwrap();
    let mut detail_isbns: Vec<&str> = detail
        .identifiers
        .iter()
        .filter(|i| i.scheme.as_deref() == Some("ISBN"))
        .map(|i| i.value.as_str())
        .collect();
    detail_isbns.sort_unstable();
    assert_eq!(detail_isbns, vec!["0000000000", "9780000000000"]);
}

/// Re-indexing the same book (the `replace_books` Removed-then-New path
/// cascade-deletes and relinks identifiers) keeps exactly the two distinct
/// rows — no accumulation, no further collapse.
#[tokio::test]
async fn reindexing_two_isbns_does_not_duplicate_identifier_rows() {
    let _covers = CoversTempDir::new("two_isbns_reindex");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let make = || {
        vec![book_with_identifiers(
            "two.epub",
            vec![
                Identifier {
                    value: "0000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )]
    };

    replace_books(&pool, "/lib", make()).await.unwrap();
    replace_books(&pool, "/lib", make()).await.unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(
        isbn_row_count(&pool, books[0].id).await,
        2,
        "reindex must not duplicate or collapse the two ISBN rows"
    );
}

/// `INSERT OR IGNORE` idempotence: an OPF that lists the exact same
/// `(scheme, value)` tuple twice collapses to a single row — the duplicate is
/// silently ignored, not a PK-violation error.
#[tokio::test]
async fn book_with_duplicate_identifier_dedups_to_one_row() {
    let _covers = CoversTempDir::new("dup_ident");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![book_with_identifiers(
            "dup.epub",
            vec![
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
                Identifier {
                    value: "9780000000000".into(),
                    scheme: Some("ISBN".into()),
                },
            ],
        )],
    )
    .await
    .expect("duplicate identifier must dedup, not error");

    let books = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(books.len(), 1);
    assert_eq!(
        isbn_row_count(&pool, books[0].id).await,
        1,
        "exact-duplicate identifier tuple must collapse to one row"
    );
}

/// Removing a single audiobook diff bucket that exceeds SQLite's 999-bind
/// parameter cap must succeed: `sync_audiobooks_removed` has to chunk the
/// `WHERE uuid IN (?, ?, ...)` list (mirroring `sync_removed` in books.rs).
/// Un-chunked, a single 1000-uuid removal would bind library_id + 1000 uuids
/// and fail at runtime with "too many SQL variables".
#[tokio::test]
async fn sync_audiobooks_with_removed_above_bind_cap_succeeds() {
    let _covers = CoversTempDir::new("ab_remove_chunk");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // 1000 audiobooks: pushes the un-chunked IN(?, ?, ...) over SQLite's
    // 999-bind cap (1 library_id + 1000 uuids = 1001 binds). 1000 also
    // forces the chunked path through two chunks (500 + 500).
    const N: usize = 1000;
    let new_books: Vec<_> = (0..N)
        .map(|i| {
            indexed_audiobook(
                &format!("Author/Book{i:04}.m4b"),
                &format!("Book {i}"),
                Some("Author"),
            )
        })
        .collect();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books,
            ..Default::default()
        },
    )
    .await
    .expect("initial sync of 1000 audiobooks should succeed");
    assert_eq!(list_books(&pool, "/lib").await.unwrap().len(), N);
    // Identity is minted (F2) — collect the durable uuids to remove from the DB.
    let all_uuids: Vec<String> = sqlx::query_scalar("SELECT uuid FROM books")
        .fetch_all(&pool)
        .await
        .unwrap();

    // Wholesale remove all 1000 in a single plan — this is the scenario
    // the issue calls out (massive library disappearing in a single scan).
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            removed_uuids: all_uuids,
            ..Default::default()
        },
    )
    .await
    .expect("wholesale removal of >500 audiobooks must not exceed bind cap");

    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "every audiobook is hidden from the grid (fileless) after wholesale removal"
    );
    let books_total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_total, fts_count,
        "every fileless audiobook keeps its books + FTS row"
    );
}

// Audiobook parts + chapters: row contents and edge cases.

#[tokio::test]
async fn sync_audiobooks_writes_all_parts_for_a_five_part_audiobook() {
    let _covers = CoversTempDir::new("ab_bulk_parts");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Book", "Big Book", Some("Author"));
    book.parts = (0..5)
        .map(|i| crate::audiobook::AudiobookPart {
            ordinal: i,
            filename: format!("Author/Book/part{i:02}.m4b"),
            size_bytes: 1000 + i,
            mtime_epoch: 100 + i,
            duration_seconds: 60.0 * (i + 1) as f64,
        })
        .collect();
    // No embedded chapters → synthetic-fallback path writes one chapter per part.
    book.chapters = vec![];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let rows: Vec<(i64, String, i64, i64, f64)> = sqlx::query_as(
        "SELECT ordinal, filename, size_bytes, mtime_epoch, duration_seconds \
         FROM book_file_parts ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(rows.len(), 5);
    for (i, (ordinal, filename, size_bytes, mtime_epoch, duration_seconds)) in
        rows.iter().enumerate()
    {
        let i = i as i64;
        assert_eq!(*ordinal, i);
        assert_eq!(filename, &format!("Author/Book/part{i:02}.m4b"));
        assert_eq!(*size_bytes, 1000 + i);
        assert_eq!(*mtime_epoch, 100 + i);
        assert_eq!(*duration_seconds, 60.0 * (i + 1) as f64);
    }
}

#[tokio::test]
async fn sync_audiobooks_writes_all_fifty_chapters_for_a_five_part_audiobook() {
    let _covers = CoversTempDir::new("ab_bulk_chapters");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Big.m4b", "Big Book", Some("Author"));
    // One 60-minute part — chapter timestamps are absolute from book start.
    book.parts = vec![crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "Author/Big.m4b".into(),
        size_bytes: 99_999,
        mtime_epoch: 500,
        duration_seconds: 3600.0,
    }];
    // 50 sequential chapters, each 60 s, with `end_ms == 0` so the gap-fill
    // branch derives the duration from the next chapter's start.
    book.chapters = (0..50)
        .map(|i| crate::audiobook::RawChapter {
            title: format!("Chapter {i}"),
            start_ms: (i as u64) * 60_000,
            end_ms: 0,
        })
        .collect();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let chapters: Vec<(i64, String, f64, f64)> = sqlx::query_as(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(chapters.len(), 50);
    for (i, (ordinal, title, start_seconds, duration_seconds)) in chapters.iter().enumerate() {
        let i = i as i64;
        assert_eq!(*ordinal, i);
        assert_eq!(title, &format!("Chapter {i}"));
        assert_eq!(*start_seconds, (i as f64) * 60.0);
        // Chapters 0..=48 fall to the gap-fill branch (next chapter's start
        // minus this chapter's start = 60 s). Chapter 49 has no next, so it
        // falls back to `total_duration - start` = 3600 - 2940 = 660 s.
        let expected = if i < 49 { 60.0 } else { 660.0 };
        assert_eq!(*duration_seconds, expected, "ordinal {i} duration mismatch");
    }
}

#[tokio::test]
async fn sync_audiobooks_writes_zero_parts_and_synthesized_chapter_for_empty_parts_edge_case() {
    let _covers = CoversTempDir::new("ab_bulk_empty");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/Only.m4b", "Only", Some("Author"));
    book.parts = vec![];
    book.chapters = vec![];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let parts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_file_parts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(parts_count, 0, "empty `parts` writes no `book_file_parts`");

    let chapters_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        chapters_count, 0,
        "no parts and no chapters → synthetic fallback is also empty"
    );
}

#[tokio::test]
async fn sync_audiobooks_writes_one_chapter_when_single_chapter_provided() {
    let _covers = CoversTempDir::new("ab_bulk_single_chap");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut book = indexed_audiobook("Author/One.m4b", "Solo", Some("Author"));
    book.parts = vec![crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "Author/One.m4b".into(),
        size_bytes: 2048,
        mtime_epoch: 42,
        duration_seconds: 120.0,
    }];
    book.chapters = vec![crate::audiobook::RawChapter {
        title: "Only".into(),
        start_ms: 0,
        end_ms: 0,
    }];

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let rows: Vec<(i64, String, f64, f64)> = sqlx::query_as(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 0);
    assert_eq!(rows[0].1, "Only");
    assert_eq!(rows[0].2, 0.0);
    // Single chapter with end_ms == 0 → duration falls back to total_duration - start = 120 s.
    assert_eq!(rows[0].3, 120.0);
}
