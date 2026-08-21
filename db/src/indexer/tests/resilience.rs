//! What a reindex does when the filesystem lies: a transiently empty root
//! and an unreadable subdirectory must leave `is_missing` flags and merged
//! uuids alone rather than wiping or resurrecting books.

use crate::pool::init_db;
use crate::test_support::{count_rows, make_test_dir, CoversTempDir};

use super::super::*;
use super::seed_ebook_at;

// ---------- #819: incomplete-enumeration data-loss guard ----------

/// Read a book's `is_missing_files` flag by its `scan_key` (the relative
/// path). Returns `None` if no such book row exists.
async fn is_missing_by_scan_key(pool: &SqlitePool, scan_key: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT is_missing_files FROM books WHERE scan_key = ?")
        .bind(scan_key)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// Drop all permissions on `dir` so `read_dir` fails with EACCES. Returns
/// `false` when the platform ignores the perm change (e.g. running as root
/// in a CI container), so the caller can skip the assertion rather than
/// report a false negative.
#[cfg(unix)]
fn make_unreadable(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(dir).is_err() {
        return true;
    }
    // The platform ignored the perm change (e.g. running as root). Restore
    // read access before bailing so the caller's temp-dir cleanup doesn't
    // trip over a 0o000 directory.
    restore_readable(dir);
    false
}

#[cfg(unix)]
fn restore_readable(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
}

/// Acceptance (a): a scan against a transiently-empty root leaves every
/// book's `is_missing_files` flag untouched. Reproduces the boot-race /
/// unmounted-NFS incident: the DB has file-backed books, but the root reads
/// empty this pass. Nothing may be flagged missing.
#[tokio::test]
async fn reindex_against_transiently_empty_root_leaves_is_missing_unchanged() {
    let _covers = CoversTempDir::new("empty-root-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("empty-root-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;

    // Simulate the mount vanishing: delete the files so the root reads
    // empty, but the DB still holds two file-backed books.
    std::fs::remove_file(lib.join("a.epub")).unwrap();
    std::fs::remove_file(lib.join("b.epub")).unwrap();

    reindex(&pool, &lib_path).await.unwrap();

    // Neither book was flagged missing — the empty read was distrusted.
    assert_eq!(
        is_missing_by_scan_key(&pool, "a.epub").await,
        Some(0),
        "a book under a transiently-empty root must not be flagged missing"
    );
    assert_eq!(is_missing_by_scan_key(&pool, "b.epub").await, Some(0));
    // Both `book_files` rows survive.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2,
        "book_files must survive a transiently-empty scan"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// Acceptance (a) + "merged_uuids sacred": an EACCES fault mid-scan must
/// leave both `is_missing_files` and `merged_uuids` unchanged — no curation
/// record is eroded as a side effect of a partial scan.
#[cfg(unix)]
#[tokio::test]
async fn reindex_with_unreadable_subdir_preserves_missing_flags_and_merged_uuids() {
    let _covers = CoversTempDir::new("eacces-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("eacces-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    // A readable book at the top, and two under a subdir we'll lock. One of
    // the locked pair is merged into the readable one, so its EPUB-format
    // `merged_uuids` row is the curation record we must protect.
    seed_ebook_at(&pool, &lib_path, "top.epub", "Top").await;
    seed_ebook_at(&pool, &lib_path, "locked/source.epub", "Source").await;
    seed_ebook_at(&pool, &lib_path, "locked/other.epub", "Other").await;

    let target = crate::test_support::uuid_by_scan_key(&pool, "top.epub").await;
    let source = crate::test_support::uuid_by_scan_key(&pool, "locked/source.epub").await;
    crate::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let merged_before = count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await;
    assert!(
        merged_before >= 1,
        "test precondition: the merge must record a merged_uuids row"
    );

    let locked = lib.join("locked");
    if !make_unreadable(&locked) {
        let _ = std::fs::remove_dir_all(&lib);
        return; // running as root — perms don't bite; skip
    }

    let result = reindex(&pool, &lib_path).await;

    restore_readable(&locked);

    result.expect("a partial scan must still succeed (it just skips removal)");

    // The un-enumerated books under `locked/` were NOT flagged missing.
    assert_eq!(
        is_missing_by_scan_key(&pool, "locked/other.epub").await,
        Some(0),
        "a book under an unreadable subdir must not be flagged missing"
    );
    // The merge's curation record survives untouched.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        merged_before,
        "a scan must never erode merged_uuids"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// Acceptance (c): forcing a full rescan (`last_indexed = 0`) while some
/// files are unreadable must not wipe `book_files` or resurrect merged-away
/// books. Even with the refresh window bypassed, the partial enumeration is
/// distrusted so the removal pass never runs.
#[cfg(unix)]
#[tokio::test]
async fn forced_full_rescan_with_unreadable_files_does_not_wipe_or_resurrect() {
    let _covers = CoversTempDir::new("forced-rescan-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("forced-rescan-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "keep.epub", "Keep").await;
    seed_ebook_at(&pool, &lib_path, "locked/source.epub", "Source").await;

    let target = crate::test_support::uuid_by_scan_key(&pool, "keep.epub").await;
    let source = crate::test_support::uuid_by_scan_key(&pool, "locked/source.epub").await;
    crate::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let books_before = count_rows(&pool, "SELECT COUNT(*) FROM books").await;
    let files_before = count_rows(&pool, "SELECT COUNT(*) FROM book_files").await;
    let merged_before = count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await;

    // Force a full rescan by resetting last_indexed to the epoch.
    sqlx::query("UPDATE scan_roots SET last_indexed = 0 WHERE path = ?")
        .bind(&lib_path)
        .execute(&pool)
        .await
        .unwrap();

    let locked = lib.join("locked");
    if !make_unreadable(&locked) {
        let _ = std::fs::remove_dir_all(&lib);
        return; // running as root — skip
    }

    let result = reindex(&pool, &lib_path).await;

    restore_readable(&locked);
    result.expect("forced rescan under a partial fault must still succeed");

    // Nothing wiped: the same book, file, and merge counts as before.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        books_before,
        "a forced rescan under a fault must not delete book rows"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        files_before,
        "a forced rescan under a fault must not drop book_files"
    );
    // The merged-away book did not resurrect (merged_uuids intact, book count
    // unchanged — no duplicate reappeared).
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        merged_before,
        "the merge must survive a forced rescan under a fault"
    );

    let _ = std::fs::remove_dir_all(&lib);
}
