//! Library paths and the `scan_roots` rows behind them: a repoint in
//! place preserves identity, still-configured and unset libraries keep
//! their data, orphan pruning, the FK cascade across the table rename, and
//! a shared root left alone by a single-slot change.

use super::super::*;
use crate::books::list_books;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

/// Repoint-in-place: changing the ebook path moves the existing
/// `scan_roots` row (keeping its id) so every book — and its durable
/// `books.uuid` — survives the move under the new path. Nothing is deleted.
#[tokio::test]
async fn set_settings_repoints_library_in_place_preserving_identity() {
    let _covers = CoversTempDir::new("repoint");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/old".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/old",
        vec![indexed(
            "a.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    let before = list_books(&pool, "/old").await.unwrap();
    assert_eq!(before.len(), 1);
    let uuid_before = before[0].unique_identifier.clone();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/new".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();

    // The old path is empty; the book now lists under the new path with its
    // identity intact, and exactly one scan_roots row / one book survive.
    assert!(list_books(&pool, "/old").await.unwrap().is_empty());
    let after = list_books(&pool, "/new").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(
        after[0].unique_identifier, uuid_before,
        "books.uuid must be preserved across a repoint"
    );
    let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(library_count, 1, "scan_roots row repointed, not recreated");
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "never-prune: the book is retained");
}

#[tokio::test]
async fn set_settings_keeps_libraries_still_configured() {
    let _covers = CoversTempDir::new("prune-keep");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/books",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();

    assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
}

/// Never-prune: clearing the ebook path does **not** delete the
/// library's books (the durable-identity safety net) — its `scan_roots` row
/// and books are retained, and re-adding the path lists them again.
#[tokio::test]
async fn set_settings_none_retains_library_data() {
    let _covers = CoversTempDir::new("prune-clear");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/books",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    set_settings(
        &pool,
        &Settings {
            ebook_library_path: None,
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();

    // The scan_roots row still owns a book, so never-prune keeps both.
    let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_roots")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        library_count, 1,
        "never-prune retains a non-empty scan root"
    );
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "books are retained when the path is cleared");
    // Re-adding the path surfaces the retained book again.
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/books".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
}

/// Never-prune: `prune_orphan_libraries` drops only **childless**
/// orphan scan roots; a root not in `keep` that still owns books is retained
/// (its books and their soft-ref user data must never be cascade-deleted),
/// and it returns no cover uuids (nothing is deleted).
#[tokio::test]
async fn prune_orphan_libraries_keeps_non_empty_roots_and_drops_childless() {
    let _covers = CoversTempDir::new("prune-never");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Root A: orphaned but owns a book → must be retained.
    let a_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind("/with-book")
    .bind("A")
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) VALUES (?, ?, ?, ?, ?)",
    )
    .bind("uuid-a")
    .bind("book.epub")
    .bind(a_id)
    .bind("/with-book")
    .bind("Kept")
    .execute(&pool)
    .await
    .unwrap();
    // Root B: orphaned and childless → must be dropped.
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind("/childless")
        .bind("B")
        .execute(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    let orphan_uuids = prune_orphan_libraries(&mut tx, &[]).await.unwrap();
    tx.commit().await.unwrap();

    assert!(
        orphan_uuids.is_empty(),
        "never-prune deletes no books, so no cover uuids are returned"
    );
    let roots: Vec<String> = sqlx::query_scalar("SELECT path FROM scan_roots ORDER BY path")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        roots,
        vec!["/with-book".to_string()],
        "childless orphan dropped, book-owning orphan retained"
    );
    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(book_count, 1, "the orphaned-root book is never deleted");
}

/// Migration 0019 renames `libraries` -> `scan_roots` via
/// `ALTER TABLE ... RENAME`. SQLite auto-rewrites the `books.library_id`
/// FK to reference the renamed table, so deleting a scan-root row must
/// still cascade-delete its books. A bug in the rename (e.g. a stray
/// table recreate that dropped the FK) would leave the book row behind.
#[tokio::test]
async fn fk_cascade_survives_libraries_rename_to_scan_roots() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let scan_root_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind("/lib")
    .bind("lib")
    .fetch_one(&pool)
    .await
    .unwrap();

    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?)")
        .bind("uuid-cascade")
        .bind(scan_root_id)
        .bind("/lib/book.epub")
        .bind("Cascade Book")
        .execute(&pool)
        .await
        .unwrap();

    sqlx::query("DELETE FROM scan_roots WHERE id = ?")
        .bind(scan_root_id)
        .execute(&pool)
        .await
        .unwrap();

    let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE library_id = ?")
        .bind(scan_root_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_count, 0,
        "deleting the scan_roots row should cascade-delete its books \
         after the libraries->scan_roots rename"
    );
}

/// Shared-root guard: when ebook + audiobook point at the same scan
/// root and only one slot's path changes, the shared `scan_roots` row must
/// NOT be repointed out from under the slot that still uses it (that would
/// silently move both libraries).
#[tokio::test]
async fn set_settings_does_not_repoint_a_shared_root_for_a_single_slot_change() {
    let _covers = CoversTempDir::new("shared-root");
    let pool = init_db("sqlite::memory:").await.unwrap();
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: Some("/lib".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();

    // Change only the ebook slot; the audiobook slot still points at "/lib".
    set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/newlib".into()),
            audiobook_library_path: Some("/lib".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();

    // The shared row is untouched (not renamed to /newlib), so the slot that
    // still uses "/lib" keeps resolving its books — nothing was moved silently.
    let paths: Vec<String> = sqlx::query_scalar("SELECT path FROM scan_roots ORDER BY path")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        paths,
        vec!["/lib".to_string()],
        "a shared root must not be repointed for a single-slot change"
    );
    assert_eq!(list_books(&pool, "/lib").await.unwrap().len(), 1);
}
