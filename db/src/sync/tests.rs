use super::*;
use crate::books::{get_book, list_books, list_indexed_rows, search_books};
use crate::covers::get_cover;
use crate::ebook::IndexedBook;
use crate::helpers::stable_uuid;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::settings::last_indexed_at;
use crate::test_support::{indexed, indexed_audiobook, indexed_with_stat, CoversTempDir};
use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

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
/// F1.7 Atrium accent round-trip. `replace_books` writes
/// `metadata.accent` into `books.accent_color`; `list_books` /
/// `get_book` / `search_books` read it back into
/// `EbookMetadata.accent`. Verify the column survives the trip and
/// `None` stays `None` (not coerced to an empty string).
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
/// #125: the write-boundary gate must accept the exact `oklch(L C H)`
/// shape the indexer emits, and reject anything else — including raw
/// hex, CSS keywords, and injection payloads that try to break out of
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
/// A Removed uuid must wipe books_fts, book_files,
/// books_authors_link, etc. — the cascade plus our explicit FTS
/// clear should leave no orphans.
#[tokio::test]
async fn sync_removes_book_cascades_links_and_fts() {
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
    assert_eq!(books_count, 0);
    assert_eq!(files_count, 0);
    assert_eq!(link_count, 0);
    assert_eq!(fts_count, 0);
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
    let gone_uuid = stable_uuid("/lib", "gone.epub");

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
    let keep_uuid = stable_uuid("/lib", "keep.epub");
    let gone_uuid = stable_uuid("/lib", "gone.epub");
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

    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1);
    assert_eq!(fts_count, 1, "FTS row count must match book count");
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
