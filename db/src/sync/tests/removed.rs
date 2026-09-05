//! The ebook Removed bucket end-to-end: a removed file ghosts its book
//! (links and FTS kept), a returning file relinks the same uuid, the cover
//! sidecar lifecycle, a whole-library removal, and a Removed batch above
//! SQLite's bind cap.

use super::super::*;
use crate::books::{list_books, list_indexed_rows};
use crate::pool::init_db;
use crate::test_support::{indexed, CoversTempDir};

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
