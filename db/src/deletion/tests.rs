//! Deletion tests: partial vs total delete, the fileless record case, the
//! attachment ledger, error variants, the impact counts, and the on-disk
//! unlink + empty-directory prune.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use super::*;
use crate::test_support::{count_rows, EnvVarGuard};

async fn pool() -> SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

/// Point every regenerable-cache directory at `dir` so a total delete's
/// cleanup can't reach the developer's real `./data` / `./covers` trees.
fn cache_env(dir: &Path) -> EnvVarGuard {
    let d = dir.as_os_str();
    EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(d))
        .also_set_os("OMNIBUS_COVERS_DIR", Some(d))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(d))
        .also_set_os("OMNIBUS_KEPUB_DIR", Some(d))
        .also_set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(d))
        .also_set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(d))
}

/// Register `root` as a scan root and return its id.
async fn seed_root(pool: &SqlitePool, root: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib') RETURNING id")
        .bind(root)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Insert a `books` row under `library_id` and return its id.
async fn seed_book(pool: &SqlitePool, library_id: i64, uuid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES (?1, ?1, ?2, '', ?3, ?3) RETURNING id",
    )
    .bind(uuid)
    .bind(library_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Insert a `book_files` row addressing `<root>/<dir>/<stem>.<format>` and
/// return its id.
async fn seed_file(
    pool: &SqlitePool,
    book_id: i64,
    root: &str,
    dir: &str,
    stem: &str,
    format: &str,
) -> i64 {
    let scan_key = format!("{dir}/{stem}.{}", format.to_lowercase());
    sqlx::query_scalar(
        "INSERT INTO book_files
             (book_id, format, filename, size_bytes, mtime_epoch, library_path, path, ordinal, scan_key)
         VALUES (?1, ?2, ?3, 1, 1, ?4, ?5, 0, ?6) RETURNING id",
    )
    .bind(book_id)
    .bind(format)
    .bind(stem)
    .bind(root)
    .bind(dir)
    .bind(&scan_key)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Create the file `seed_file` points at, parent directories included.
fn write_file(root: &Path, dir: &str, stem: &str, format: &str) -> PathBuf {
    let parent = root.join(dir);
    std::fs::create_dir_all(&parent).unwrap();
    let path = parent.join(format!("{stem}.{}", format.to_lowercase()));
    std::fs::write(&path, b"book").unwrap();
    path
}

fn temp_dir(tag: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "omnibus_delete_{tag}_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

// --- partial delete --------------------------------------------------------

#[tokio::test]
async fn delete_book_items_removes_one_file_and_retains_the_book_when_others_remain() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Dual Format").await;
    let epub = seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;
    seed_file(&pool, book, "/lib", "a", "a", "M4B").await;

    let out = delete_book_items(&pool, "uuid-a", &[epub], &[])
        .await
        .unwrap();

    assert_eq!(out.deleted_file_ids, vec![epub]);
    assert!(!out.book_deleted);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        1
    );
}

#[tokio::test]
async fn delete_book_items_is_a_no_op_when_nothing_is_selected_on_a_filed_book() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Kept").await;
    seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;

    let out = delete_book_items(&pool, "uuid-a", &[], &[]).await.unwrap();

    assert!(out.deleted_file_ids.is_empty());
    assert!(!out.book_deleted);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        1
    );
}

// --- total delete ----------------------------------------------------------

#[tokio::test]
async fn delete_book_items_purges_the_book_and_its_user_data_when_every_item_goes() {
    let dir = temp_dir("purge");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Doomed").await;
    let file = seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;
    let user = seed_user(&pool, "reader").await;
    let copy = seed_user_data(&pool, user, "uuid-a").await;

    let out = delete_book_items(&pool, "uuid-a", &[file], &[copy])
        .await
        .unwrap();

    assert!(out.book_deleted);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books_fts").await, 0);
    for table in [
        "annotations",
        "bookmarks",
        "user_ratings",
        "journal_entries",
        "physical_copies",
        "reading_progress",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table}");
        assert_eq!(count_rows(&pool, &sql).await, 0, "{table} not purged");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// #1395: a total delete must also remove the book's cached rewritten
/// export EPUB, alongside the thumbnail / HLS / KEPUB caches it already
/// cleans up — otherwise the file lingers on disk unreferenced forever.
#[tokio::test]
async fn delete_book_items_removes_the_export_epub_cache_on_total_delete() {
    let dir = temp_dir("export_epub");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Doomed").await;
    let file = seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;

    let cache_path = crate::epub_rewrite::export_epub_path(book);
    std::fs::write(&cache_path, b"stale rewritten epub").unwrap();

    delete_book_items(&pool, "uuid-a", &[file], &[])
        .await
        .unwrap();

    assert!(
        !cache_path.exists(),
        "total delete must remove the cached export EPUB"
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn delete_book_items_keeps_the_record_when_a_physical_copy_is_left_behind() {
    let root = temp_dir("keeprecord");
    let _env = cache_env(&root);
    let pool = pool().await;
    let root_str = root.to_str().unwrap().to_string();
    let lib = seed_root(&pool, &root_str).await;
    let book = seed_book(&pool, lib, "uuid-a", "Also On My Shelf").await;
    let file = seed_file(&pool, book, &root_str, "a", "a", "EPUB").await;
    write_file(&root, "a", "a", "EPUB");
    let user = seed_user(&pool, "reader").await;
    crate::physical::add_physical_copy(&pool, "uuid-a", None, Some(user), None)
        .await
        .unwrap();

    // Every *file* goes, but the copy stays — so the book must too.
    let out = delete_book_items(&pool, "uuid-a", &[file], &[])
        .await
        .unwrap();

    assert!(!out.book_deleted);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0
    );
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn delete_book_items_deletes_the_record_when_its_last_physical_copy_goes() {
    let dir = temp_dir("fileless");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    seed_book(&pool, lib, "uuid-p", "Physical Only").await;
    let user = seed_user(&pool, "reader").await;
    let copy = crate::physical::add_physical_copy(&pool, "uuid-p", None, Some(user), None)
        .await
        .unwrap();

    let out = delete_book_items(&pool, "uuid-p", &[], &[copy.id])
        .await
        .unwrap();

    assert!(out.book_deleted);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM physical_copies").await,
        0
    );
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn delete_book_items_still_purges_the_book_when_an_id_is_sent_twice() {
    let dir = temp_dir("dupes");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Doubled").await;
    let file = seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;

    // A retry (or a client bug) repeating an id must not inflate the selection
    // count past the item count and quietly skip the purge.
    let out = delete_book_items(&pool, "uuid-a", &[file, file], &[])
        .await
        .unwrap();

    assert!(out.book_deleted);
    assert_eq!(out.deleted_file_ids, vec![file]);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn delete_book_items_deletes_the_record_of_a_book_with_no_items_at_all() {
    let dir = temp_dir("ghost");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    seed_book(&pool, lib, "uuid-g", "Ghost").await;

    let out = delete_book_items(&pool, "uuid-g", &[], &[]).await.unwrap();

    assert!(out.book_deleted);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn delete_book_items_prunes_the_emptied_directories_under_the_library_root() {
    let root = temp_dir("prune");
    let _env = cache_env(&root);
    let pool = pool().await;
    let root_str = root.to_str().unwrap().to_string();
    let lib = seed_root(&pool, &root_str).await;
    let book = seed_book(&pool, lib, "uuid-a", "Nested").await;
    let file = seed_file(&pool, book, &root_str, "author/title", "title", "EPUB").await;
    let path = write_file(&root, "author/title", "title", "EPUB");

    delete_book_items(&pool, "uuid-a", &[file], &[])
        .await
        .unwrap();

    assert!(!path.exists(), "file survived the delete");
    assert!(!root.join("author/title").exists(), "title dir survived");
    assert!(!root.join("author").exists(), "author dir survived");
    assert!(root.exists(), "library root must never be pruned");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn delete_book_items_removes_the_per_stem_sidecar_cover_but_not_a_shared_one() {
    let root = temp_dir("sidecar");
    let _env = cache_env(&root);
    let pool = pool().await;
    let root_str = root.to_str().unwrap().to_string();
    let lib = seed_root(&pool, &root_str).await;
    let book = seed_book(&pool, lib, "uuid-a", "Sidecar").await;
    let epub = seed_file(&pool, book, &root_str, "flat", "piranesi", "EPUB").await;
    write_file(&root, "flat", "piranesi", "EPUB");
    // A cover named after the book, and a folder-level one a flat-dump layout
    // shares with every other book in the same directory.
    let own = write_file(&root, "flat", "piranesi", "PNG");
    let shared = write_file(&root, "flat", "cover", "JPG");

    delete_book_items(&pool, "uuid-a", &[epub], &[])
        .await
        .unwrap();

    assert!(!own.exists(), "per-stem sidecar cover survived");
    assert!(shared.exists(), "shared folder cover must not be deleted");
    std::fs::remove_dir_all(&root).ok();
}

#[tokio::test]
async fn delete_book_items_keeps_a_directory_that_still_holds_another_file() {
    let root = temp_dir("keepdir");
    let _env = cache_env(&root);
    let pool = pool().await;
    let root_str = root.to_str().unwrap().to_string();
    let lib = seed_root(&pool, &root_str).await;
    let book = seed_book(&pool, lib, "uuid-a", "Two Formats").await;
    let epub = seed_file(&pool, book, &root_str, "author/title", "title", "EPUB").await;
    seed_file(&pool, book, &root_str, "author/title", "title", "M4B").await;
    write_file(&root, "author/title", "title", "EPUB");
    let kept = write_file(&root, "author/title", "title", "M4B");

    delete_book_items(&pool, "uuid-a", &[epub], &[])
        .await
        .unwrap();

    assert!(kept.exists(), "sibling file was removed");
    assert!(root.join("author/title").exists(), "non-empty dir pruned");
    std::fs::remove_dir_all(&root).ok();
}

// --- attachments -----------------------------------------------------------

#[tokio::test]
async fn delete_book_items_drops_the_merged_uuids_guard_of_the_deleted_file() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Attached").await;
    seed_file(&pool, book, "/lib", "a", "a", "EPUB").await;
    let attached = seed_file(&pool, book, "/audio", "a", "a", "M4B").await;
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('uuid-audio', ?, 'M4B', '/audio', 'a/a.m4b')",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    delete_book_items(&pool, "uuid-a", &[attached], &[])
        .await
        .unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0
    );
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn delete_book_items_resolves_a_ledger_uuid_to_its_target_book() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Attached").await;
    let attached = seed_file(&pool, book, "/audio", "a", "a", "M4B").await;
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('uuid-audio', ?, 'M4B', '/audio', 'a/a.m4b')",
    )
    .bind(book)
    .execute(&pool)
    .await
    .unwrap();

    // Addressed by the ledger uuid, not the book's own — the file-picker
    // surfaces attached formats under the same page.
    let out = delete_book_items(&pool, "uuid-audio", &[attached], &[])
        .await
        .unwrap();

    assert!(out.book_deleted, "last file went, so the book should too");
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
}

// --- concurrency -------------------------------------------------------

#[tokio::test]
async fn delete_book_items_never_orphans_a_zero_item_book_row_under_concurrent_disjoint_deletes() {
    // Regression test for the TOCTOU this module used to have: two concurrent
    // calls deleting disjoint files must not both read the pre-delete item
    // count and both decide `total_delete = false` — that would leave
    // `book_files` at 0 rows with the `books` row never purged, a state
    // nothing else cleans up. `BEGIN IMMEDIATE` serializes the two calls on
    // the RESERVED write lock, so whichever runs second recomputes the count
    // fresh (against the first call's already-committed delete) and sees the
    // true remaining total.
    //
    // `tokio::spawn` scheduling isn't guaranteed to interleave the two calls
    // at the exact snapshot-vs-commit window a regression would need to slip
    // through — a single run could pass by luck even on a reintroduced bug.
    // Repeating the race against a fresh book each time makes a regression
    // extremely likely to surface within the loop.
    let dir = temp_dir("concurrent");
    let _env = cache_env(&dir);
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;

    for i in 0..25 {
        let uuid = format!("uuid-race-{i}");
        let book = seed_book(&pool, lib, &uuid, "Raced").await;
        let a = seed_file(&pool, book, "/lib", "a", &format!("a{i}"), "EPUB").await;
        let b = seed_file(&pool, book, "/lib", "a", &format!("a{i}"), "M4B").await;

        let p1 = pool.clone();
        let p2 = pool.clone();
        let u1 = uuid.clone();
        let u2 = uuid.clone();
        let t1 = tokio::spawn(async move { delete_book_items(&p1, &u1, &[a], &[]).await });
        let t2 = tokio::spawn(async move { delete_book_items(&p2, &u2, &[b], &[]).await });

        let r1 = t1.await.unwrap().unwrap();
        let r2 = t2.await.unwrap().unwrap();

        // Whichever call ran second sees the other's file already gone, so
        // exactly one of the two — never both, never neither — purges the
        // book.
        assert_eq!(
            [r1.book_deleted, r2.book_deleted]
                .iter()
                .filter(|d| **d)
                .count(),
            1,
            "exactly one of the two racing deletes must purge the now-empty book (iteration {i})"
        );
        assert_eq!(
            count_rows(
                &pool,
                &format!("SELECT COUNT(*) FROM books WHERE uuid = '{uuid}'")
            )
            .await,
            0,
            "iteration {i}"
        );
    }
    std::fs::remove_dir_all(&dir).ok();
}

// --- DeleteError variants ------------------------------------------------

#[tokio::test]
async fn delete_book_items_wraps_a_books_error_when_the_pool_is_closed() {
    // `resolve_book_id_by_uuid` is the first DB touch in `delete_book_items`,
    // so closing the pool up front guarantees it's the call that fails,
    // giving a deterministic `DeleteError::Books`.
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    seed_book(&pool, lib, "uuid-a", "Doomed").await;
    pool.close().await;

    let err = delete_book_items(&pool, "uuid-a", &[], &[])
        .await
        .unwrap_err();

    assert!(matches!(err, DeleteError::Books(_)));
}

#[tokio::test]
async fn canonical_uuid_propagates_a_sqlx_error_when_the_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;

    let err = canonical_uuid(&pool, 1).await.unwrap_err();

    assert!(matches!(err, DeleteError::Sqlx(_)));
}

#[tokio::test]
async fn delete_error_wraps_a_physical_error_when_the_copy_lookup_fails() {
    // Both `delete_book_items` and `book_deletion_manifest` convert a
    // `physical::PhysicalError` via `?`. Isolating that specific call inside
    // either multi-step orchestrator isn't possible with a closed-pool
    // trigger alone (an earlier DB touch would fail first and mask it), so
    // this drives the conversion directly against the same real DB failure.
    let pool = pool().await;
    pool.close().await;

    let physical_err = crate::physical::list_physical_copies(&pool, "uuid-a")
        .await
        .unwrap_err();
    let err: DeleteError = physical_err.into();

    assert!(matches!(err, DeleteError::Physical(_)));
}

// --- errors ----------------------------------------------------------------

#[tokio::test]
async fn delete_book_items_errors_when_the_book_is_unknown() {
    let pool = pool().await;

    let err = delete_book_items(&pool, "nope", &[], &[])
        .await
        .unwrap_err();

    assert!(matches!(err, DeleteError::BookNotFound));
}

#[tokio::test]
async fn delete_book_items_errors_when_a_file_belongs_to_another_book() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let a = seed_book(&pool, lib, "uuid-a", "A").await;
    let b = seed_book(&pool, lib, "uuid-b", "B").await;
    seed_file(&pool, a, "/lib", "a", "a", "EPUB").await;
    let other = seed_file(&pool, b, "/lib", "b", "b", "EPUB").await;

    let err = delete_book_items(&pool, "uuid-a", &[other], &[])
        .await
        .unwrap_err();

    assert!(matches!(err, DeleteError::FileNotFound(id) if id == other));
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );
}

#[tokio::test]
async fn delete_book_items_errors_when_a_copy_belongs_to_another_book() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    seed_book(&pool, lib, "uuid-a", "A").await;
    seed_book(&pool, lib, "uuid-b", "B").await;
    let user = seed_user(&pool, "reader").await;
    let other = crate::physical::add_physical_copy(&pool, "uuid-b", None, Some(user), None)
        .await
        .unwrap();

    let err = delete_book_items(&pool, "uuid-a", &[], &[other.id])
        .await
        .unwrap_err();

    assert!(matches!(err, DeleteError::CopyNotFound(id) if id == other.id));
}

// --- manifest --------------------------------------------------------------

#[tokio::test]
async fn book_deletion_manifest_lists_every_item_and_counts_the_user_data() {
    let pool = pool().await;
    let lib = seed_root(&pool, "/lib").await;
    let book = seed_book(&pool, lib, "uuid-a", "Loved").await;
    seed_file(&pool, book, "/lib", "clarke", "piranesi", "EPUB").await;
    let user = seed_user(&pool, "reader").await;
    seed_user_data(&pool, user, "uuid-a").await;

    let manifest = book_deletion_manifest(&pool, "uuid-a").await.unwrap();

    assert_eq!(manifest.files.len(), 1);
    // The row carries its library-relative path, which the dialog renders.
    assert_eq!(
        manifest.files[0].path.as_deref(),
        Some("clarke/piranesi.epub")
    );
    assert_eq!(manifest.copies.len(), 1);
    assert_eq!(manifest.impact.highlights, 1);
    assert_eq!(manifest.impact.journal_entries, 1);
    assert_eq!(manifest.impact.bookmarks, 1);
    assert_eq!(manifest.impact.ratings, 1);
}

#[tokio::test]
async fn book_deletion_manifest_errors_when_the_book_is_unknown() {
    let pool = pool().await;

    let err = book_deletion_manifest(&pool, "nope").await.unwrap_err();

    assert!(matches!(err, DeleteError::BookNotFound));
}

// --- fixtures --------------------------------------------------------------

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO users (username, password_hash) VALUES (?, 'x') RETURNING id")
        .bind(username)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// One row in each user-data table the purge is responsible for, plus a
/// physical copy. Returns the copy's id, since it counts as a deletable item.
async fn seed_user_data(pool: &SqlitePool, user_id: i64, uuid: &str) -> i64 {
    for sql in [
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi, updated_at)
         VALUES (?1, ?2, 'epub', 'epubcfi(/2)', 1)",
        "INSERT INTO bookmarks (user_id, book_uuid, position, created_at)
         VALUES (?1, ?2, 'x', 1)",
        "INSERT INTO annotations (user_id, book_uuid, epub_cfi_range, created_at)
         VALUES (?1, ?2, 'x', 1)",
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?1, ?2, 8, 1)",
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, created_at, updated_at)
         VALUES (?1, ?2, 'note', 1, 1)",
    ] {
        sqlx::query(sql)
            .bind(user_id)
            .bind(uuid)
            .execute(pool)
            .await
            .unwrap();
    }
    crate::physical::add_physical_copy(pool, uuid, None, Some(user_id), None)
        .await
        .unwrap()
        .id
}
