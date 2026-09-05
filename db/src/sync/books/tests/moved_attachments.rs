//! The Moved bucket for attached and two-epub books: the `merged_uuids`
//! ledger is repointed while the target `books` row stays put, a missing
//! location override is minted, each file relocates only its own row,
//! replays are idempotent, and a taken ledger scan key is skipped.

use sqlx::SqlitePool;

use super::super::{sync_books, SyncPlan};
use super::{book_files_count, seed_book_with_file, seed_scan_root};
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{count_rows, CoversTempDir};

/// AC7: moving a cross-format attachment updates its own `merged_uuids`
/// and `book_files` rows — including the location override, which is what
/// the read path resolves the file through — and leaves the target book's
/// own `scan_key` / `path` (which name a *different* file) untouched.
#[tokio::test]
async fn moved_attachment_updates_the_ledger_and_leaves_the_target_books_row_alone() {
    let _covers = CoversTempDir::new("sync_moved_attachment");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    // Plant an M4B attachment on the same book, keyed on its own path.
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, library_path, path, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', '/lib', 'Audio/Old', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "ledger-uuid".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let ledger_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM merged_uuids WHERE uuid = 'ledger-uuid'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ledger_scan_key, "Audio/New/book.m4b",
        "AC7: the ledger follows its file"
    );
    let (file_scan_key, override_path): (String, String) = sqlx::query_as(
        "SELECT scan_key, path FROM book_files WHERE book_id = ? AND format = 'M4B'",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(file_scan_key, "Audio/New/book.m4b");
    assert_eq!(
        override_path, "Audio/New",
        "AC7: an attachment's location override follows it too"
    );
    let (target_scan_key, target_path): (String, String) =
        sqlx::query_as("SELECT scan_key, path FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        target_scan_key, "Ebooks/book.epub",
        "AC7: the target book's own scan_key names a different file and must not move"
    );
    assert_eq!(target_path, "Ebooks");
}

/// AC8: for a book holding two EPUBs (via `merge_books`), moving either
/// file is detected and written independently — the anchor moves through
/// `books`, the merged-in file through its ledger row, and neither
/// disturbs the other.
#[tokio::test]
async fn moved_file_of_a_two_epub_book_relocates_only_its_own_row() {
    let _covers = CoversTempDir::new("sync_moved_two_epub");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (anchor_uuid, merged_uuid, book_id) = seed_merged_two_epub_book(&pool).await;

    // Move the merged-in file only.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: merged_uuid.clone(),
                filename: "Moved/two.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        anchor_scan_key, "A/one.epub",
        "AC8: the sibling anchor row is left untouched"
    );
    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["A/one.epub".to_string(), "Moved/two.epub".to_string()],
        "AC8: only the moved file's own book_files row relocated"
    );

    // Now move the anchor file too, in a second scan.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: anchor_uuid,
                filename: "Moved/one.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(anchor_scan_key, "Moved/one.epub", "AC8: the anchor moved");
    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["Moved/one.epub".to_string(), "Moved/two.epub".to_string()],
    );
}

/// AC8, the both-at-once half: relocating each file of a two-EPUB book in
/// one scan lands both.
#[tokio::test]
async fn moving_both_files_of_a_two_epub_book_in_one_scan_relocates_both() {
    let _covers = CoversTempDir::new("sync_moved_two_epub_both");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (anchor_uuid, merged_uuid, book_id) = seed_merged_two_epub_book(&pool).await;

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![
                crate::sync::MovedFile {
                    uuid: anchor_uuid,
                    filename: "Moved/one.epub".into(),
                },
                crate::sync::MovedFile {
                    uuid: merged_uuid,
                    filename: "Moved/two.epub".into(),
                },
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["Moved/one.epub".to_string(), "Moved/two.epub".to_string()],
    );
    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(anchor_scan_key, "Moved/one.epub");
}

/// Seed the two-EPUB shape through the real `merge_books` path and return
/// `(anchor uuid, merged-in ledger uuid, surviving books.id)`.
async fn seed_merged_two_epub_book(pool: &SqlitePool) -> (String, String, i64) {
    let library_id = seed_scan_root(pool).await;
    let target_id = seed_book_with_file(pool, library_id, "A/one.epub").await;
    let source_id = seed_book_with_file(pool, library_id, "B/two.epub").await;
    let anchor_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let source_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    crate::merge::merge_books(pool, &source_uuid, &anchor_uuid, None)
        .await
        .unwrap();
    (anchor_uuid, source_uuid, target_id)
}

/// Every `book_files.scan_key` under one book, sorted.
async fn scan_keys_for(pool: &SqlitePool, book_id: i64) -> Vec<String> {
    let mut keys: Vec<String> =
        sqlx::query_scalar("SELECT COALESCE(scan_key, '') FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_all(pool)
            .await
            .unwrap();
    keys.sort();
    keys
}

/// End-to-end through the real classifier: three consecutive reindexes
/// after a move converge, and the move never reaches Removed — so the
/// mass-missing breaker never sees it (AC3) and no ledger row is minted
/// (AC1).
#[tokio::test]
async fn three_reindexes_after_a_stat_matched_move_converge_without_ghosting() {
    let _covers = CoversTempDir::new("sync_moved_convergence");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    // Give the seeded file a real stat pair so it can be matched at all.
    sqlx::query("UPDATE book_files SET mtime_epoch = 4242, size_bytes = 9999 WHERE book_id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let disk = [crate::ebook::StatEntry {
        filename: "New/book.epub".into(),
        scan_key: "New/book.epub".into(),
        mtime_epoch: 4242,
        size_bytes: 9999,
        error: None,
    }];

    for pass in 0..3 {
        let db_rows = crate::books::list_indexed_rows_for_formats(&pool, "/lib", &["EPUB"])
            .await
            .unwrap();
        let diff = diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
        if pass == 0 {
            assert_eq!(diff.moved.len(), 1, "pass 0: the move is detected");
            assert!(diff.removed.is_empty(), "AC3: nothing ghosts");
            assert!(diff.new.is_empty(), "AC1: no Phase-B parse target");
        } else {
            assert!(diff.moved.is_empty(), "pass {pass}: already settled");
            assert_eq!(diff.unchanged.len(), 1, "pass {pass}: classifies Unchanged");
        }

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                moved: diff.moved,
                removed_uuids: diff.removed,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(book_files_count(&pool, book_id).await, 1, "pass {pass}");
        let (uuid, scan_key, is_missing): (String, String, i64) =
            sqlx::query_as("SELECT uuid, scan_key, is_missing_files FROM books WHERE id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(uuid, uuid_before, "pass {pass}: uuid preserved");
        assert_eq!(scan_key, "New/book.epub", "pass {pass}: scan_key settled");
        assert_eq!(is_missing, 0, "pass {pass}: never flagged missing");
        assert_eq!(
            count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
            0,
            "pass {pass}: no ledger row minted"
        );
    }
}

/// An attachment's `book_files.path` override is the *only* column that
/// points at its file — the target book's `books.path` names a different
/// one — so the relocation must write it even on a row that somehow
/// carries none, rather than leaving the file inheriting a directory it
/// does not live in.
#[tokio::test]
async fn moved_attachment_without_a_location_override_gains_one_pointing_at_its_new_home() {
    let _covers = CoversTempDir::new("sync_moved_attach_null_path");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "ledger-uuid".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let override_path: Option<String> =
        sqlx::query_scalar("SELECT path FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        override_path.as_deref(),
        Some("Audio/New"),
        "the attachment must name its own directory, never inherit the target's"
    );
}

/// Replaying the same Moved plan is a no-op: the second pass reads the
/// already-moved `scan_key` as its "old" value and rewrites the same
/// values. Worth pinning — a sync plan is data, and a retried write path
/// must not corrupt a book that already landed.
#[tokio::test]
async fn replaying_the_same_moved_plan_is_idempotent() {
    let _covers = CoversTempDir::new("sync_moved_idempotent");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    for pass in 0..2 {
        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                moved: vec![crate::sync::MovedFile {
                    uuid: uuid.clone(),
                    filename: "New/book.epub".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (scan_key, path): (String, String) =
            sqlx::query_as("SELECT scan_key, path FROM books WHERE id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(scan_key, "New/book.epub", "pass {pass}");
        assert_eq!(path, "New", "pass {pass}");
        assert_eq!(book_files_count(&pool, book_id).await, 1, "pass {pass}");
        assert_eq!(
            scan_keys_for(&pool, book_id).await,
            vec!["New/book.epub".to_string()],
            "pass {pass}"
        );
    }
}

/// A moved attachment must not land on a `(library_path, scan_key)` another
/// ledger row already holds. `merged_uuids` has only a non-unique index
/// there, so the UPDATE cannot abort the transaction — it would silently
/// leave two rows sharing one key, and `find_attachment_by_scan_key` uses
/// `fetch_optional`, so a later scan would resolve the attachment
/// arbitrarily. Reachable because a ledger row whose `book_files` row was
/// dropped (the Changed path wipes a book's rows per format) is excluded
/// from the diff's merged projection, so its path is offered as an arrival.
/// Mirrors the `books`-side `idx_books_scan_key` pre-check.
#[tokio::test]
async fn moved_attachment_onto_a_scan_key_another_ledger_row_holds_is_skipped() {
    let _covers = CoversTempDir::new("sync_moved_ledger_collision");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    // The attachment that is about to move, with its backing file row.
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, library_path, path, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', '/lib', 'Audio/Old', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('mover', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    // A stale ledger row already holding the destination — no `book_files`
    // row behind it, which is exactly why the diff can offer that path.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('squatter', ?, 'M4B', '/lib', 'Audio/New/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "mover".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let sharing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM merged_uuids WHERE library_path = '/lib'
           AND scan_key = 'Audio/New/book.m4b'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        sharing, 1,
        "two ledger rows must never share one (library_path, scan_key)"
    );
    let mover_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM merged_uuids WHERE uuid = 'mover'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        mover_scan_key, "Audio/Old/book.m4b",
        "the refused relocation left the ledger row alone"
    );
    let file_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        file_scan_key, "Audio/Old/book.m4b",
        "declining writes nothing at all — not even the file row"
    );
}
