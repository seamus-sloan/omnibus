//! The shared row writers — `insert_new_audiobook`,
//! `try_attach_new_audiobook` (ledger and title/author match paths, the
//! wishlist promotion), `attach_audiobook_file` — plus
//! `backfill_audiobook_stats` and `stamp_audiobooks_last_indexed`, each
//! with its DB-failure path.

use std::collections::HashSet;

use super::super::backfill_audiobook_stats;
use super::super::shared::{attach_audiobook_file, insert_new_audiobook, try_attach_new_audiobook};
use super::super::stamp_audiobooks_last_indexed;
use super::{book_files_count, seed_audiobook_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::sync::books::SyncError;
use crate::test_support::{count_rows, indexed_audiobook, seed_synced_ebook, CoversTempDir};

/// `insert_new_audiobook` writes the canonical `books`/`book_files` rows
/// plus parts, chapters, and the author link, and pushes the cover triple
/// when the group carries one.
#[tokio::test]
async fn insert_new_audiobook_writes_books_book_files_parts_chapters_and_author_link() {
    let _covers = CoversTempDir::new("ab_insert_new_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut b = indexed_audiobook("Author/Solo.m4b", "Solo", Some("Solo Author"));
    b.cover = Some(("image/png".into(), vec![1, 2, 3]));

    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(covers.len(), 1, "the cover triple is pushed");
    let book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'Author/Solo.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(book_files_count(&pool, book_id).await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1
    );
}

/// `insert_new_audiobook` propagates the underlying `sqlx::Error` as
/// `SyncError::Db` when a downstream write (here, the parts insert) hits a
/// missing table.
#[tokio::test]
async fn insert_new_audiobook_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let b = indexed_audiobook("Author/Broken.m4b", "Broken", Some("Author"));

    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_file_parts")
        .execute(&mut *tx)
        .await
        .unwrap();
    let mut covers = Vec::new();
    let err = insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

/// `try_attach_new_audiobook` attaches unconditionally when the file's
/// `scan_key` is already recorded in `merged_uuids` — even though titles no
/// longer need to match, since the ledger hit is checked first.
#[tokio::test]
async fn try_attach_new_audiobook_attaches_via_recorded_ledger_entry() {
    let _covers = CoversTempDir::new("ab_try_attach_ledger");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Stoker/Dracula.m4b')",
    )
    .bind(target_id)
    .execute(&pool)
    .await
    .unwrap();

    // Title no longer matches — the ledger hit must attach anyway.
    let b = indexed_audiobook(
        "Stoker/Dracula.m4b",
        "A Totally Different Title",
        Some("Someone Else"),
    );
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        attached,
        "the ledger entry attaches regardless of title drift"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no second books row was created"
    );
    assert_eq!(book_files_count(&pool, target_id).await, 2);
}

/// `try_attach_new_audiobook` attaches via the title+author heuristic when
/// exactly one existing book matches and lacks this format — the
/// no-ledger-entry path.
#[tokio::test]
async fn try_attach_new_audiobook_attaches_via_title_author_match_when_no_ledger_entry() {
    let _covers = CoversTempDir::new("ab_try_attach_heuristic");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(attached);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(book_files_count(&pool, target_id).await, 2);
}

/// `try_attach_new_audiobook` returns `Ok(false)` without writing when no
/// author is present — too weak a signal to auto-match, per the doc
/// comment.
#[tokio::test]
async fn try_attach_new_audiobook_declines_when_no_author_present() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let b = indexed_audiobook("Author/NoAuthor.m4b", "No Author", None);
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(!attached);
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
}

/// `try_attach_new_audiobook` propagates a `sqlx::Error` from the
/// title+author lookup (`find_attach_target`, which joins `book_files`) as
/// `SyncError::Db`.
#[tokio::test]
async fn try_attach_new_audiobook_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let b = indexed_audiobook("Author/Broken.m4b", "Broken", Some("Author"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_files")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

/// `attach_audiobook_file` writes the attached file's `book_files`/parts/
/// chapters rows under the target book, records the `merged_uuids` ledger
/// row, and adopts the cover when the target has none.
#[tokio::test]
async fn attach_audiobook_file_writes_rows_and_ledger_and_adopts_cover() {
    let _covers = CoversTempDir::new("ab_attach_file_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    b.cover = Some(("image/png".into(), vec![9, 9, 9]));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(book_files_count(&pool, target_id).await, 2);
    let (mu_book, mu_lib): (i64, String) =
        sqlx::query_as("SELECT book_id, library_path FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mu_book, target_id);
    assert_eq!(mu_lib, "/lib");
    let book_file_id: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(target_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = {book_file_id}")
        )
        .await,
        1
    );
    assert_eq!(
        covers.len(),
        1,
        "the target had no cover, so the attached file's cover is adopted"
    );
}

/// Re-attaching a part of an already-attached multi-part file (a stat
/// change) preserves its prior `ordinal` slot rather than jumping to the
/// end of the picker order: with 3 attached M4B parts (ordinals 0,1,2),
/// re-attaching the middle one must not let `COALESCE(MAX(ordinal)+1, ...)`
/// recompute against only the surviving siblings (which would yield 3, not 1).
#[tokio::test]
async fn attach_audiobook_file_preserves_prior_ordinal_on_reattach() {
    let _covers = CoversTempDir::new("ab_attach_file_ordinal");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    for part in [
        "Stoker/Dracula-1.m4b",
        "Stoker/Dracula-2.m4b",
        "Stoker/Dracula-3.m4b",
    ] {
        let b = indexed_audiobook(part, "Dracula", Some("Bram Stoker"));
        let mut covers = Vec::new();
        let mut tx = pool.begin().await.unwrap();
        attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
            .await
            .unwrap();
        tx.commit().await.unwrap();
    }
    let ordinal_before: i64 = sqlx::query_scalar(
        "SELECT ordinal FROM book_files
          WHERE book_id = ? AND format = 'M4B' AND scan_key = 'Stoker/Dracula-2.m4b'",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(ordinal_before, 1, "the middle part holds slot 1");

    // Re-attach the middle part (e.g. a stat change) — its ordinal slot
    // must be preserved, not recomputed against the surviving siblings.
    let mut re_b = indexed_audiobook("Stoker/Dracula-2.m4b", "Dracula", Some("Bram Stoker"));
    re_b.max_mtime_epoch = 999;
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &re_b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let ordinal_after: i64 = sqlx::query_scalar(
        "SELECT ordinal FROM book_files
          WHERE book_id = ? AND format = 'M4B' AND scan_key = 'Stoker/Dracula-2.m4b'",
    )
    .bind(target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        ordinal_after, ordinal_before,
        "the middle slot survives re-attach rather than jumping to the end"
    );
}

/// `attach_audiobook_file` propagates a `sqlx::Error` as `SyncError::Db`
/// when the parts insert hits a missing table.
#[tokio::test]
async fn attach_audiobook_file_propagates_db_error_when_table_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid =
        seed_synced_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let b = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE book_file_parts")
        .execute(&mut *tx)
        .await
        .unwrap();
    let err = attach_audiobook_file(&mut tx, target_id, "M4B", "/lib", &b, &mut covers)
        .await
        .unwrap_err();
    assert!(matches!(err, SyncError::Db(_)));
}

/// `backfill_audiobook_stats` UPDATEs only the `book_files.(mtime_epoch,
/// size_bytes)` columns for the given uuid — no re-parse, no link writes.
#[tokio::test]
async fn backfill_audiobook_stats_updates_mtime_and_size_for_matching_uuid() {
    let _covers = CoversTempDir::new("ab_backfill_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Author/Book.m4b").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let mut tx = pool.begin().await.unwrap();
    backfill_audiobook_stats(&mut tx, library_id, &[(uuid, 555, 777)])
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let (mtime, size): (i64, i64) =
        sqlx::query_as("SELECT mtime_epoch, size_bytes FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mtime, 555);
    assert_eq!(size, 777);
}

/// `backfill_audiobook_stats` is a no-op for an empty batch.
#[tokio::test]
async fn backfill_audiobook_stats_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    backfill_audiobook_stats(&mut tx, library_id, &[])
        .await
        .unwrap();
    tx.commit().await.unwrap();
}

/// `stamp_audiobooks_last_indexed` stamps `scan_roots.last_indexed` with a
/// non-zero wall-clock value.
#[tokio::test]
async fn stamp_audiobooks_last_indexed_sets_scan_roots_last_indexed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let before: Option<i64> =
        sqlx::query_scalar("SELECT last_indexed FROM scan_roots WHERE id = ?")
            .bind(library_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(before, None, "unstamped before the call");

    let mut tx = pool.begin().await.unwrap();
    stamp_audiobooks_last_indexed(&mut tx, library_id)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let after: Option<i64> = sqlx::query_scalar("SELECT last_indexed FROM scan_roots WHERE id = ?")
        .bind(library_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        after.unwrap_or(0) > 0,
        "last_indexed stamped with wall-clock seconds"
    );
}

/// Audiobook twin of the ebook attach-promotion test: attaching an M4B to a
/// fileless wishlist book must move it off the `physical://local` pseudo-root
/// so the path-scoped reads surface it.
#[tokio::test]
async fn try_attach_new_audiobook_promotes_a_wishlist_target_off_the_physical_root() {
    let _covers = CoversTempDir::new("ab_attach_promotes_wishlist");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let wishlist_uuid = crate::physical::create_fileless_book(
        &pool,
        crate::physical::FilelessBook {
            title: "Seeded".into(),
            authors: vec!["Seed Author".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    let lib_id = seed_scan_root(&pool).await;

    let b = indexed_audiobook("Seed Author/Seeded.m4b", "Seeded", Some("Seed Author"));
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = try_attach_new_audiobook(&mut tx, "/lib", &b, &HashSet::new(), &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(attached, "the group must attach to the wishlist book");
    let library_id: i64 = sqlx::query_scalar("SELECT library_id FROM books WHERE uuid = ?")
        .bind(&wishlist_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        library_id, lib_id,
        "attach must promote off the pseudo-root"
    );
}
