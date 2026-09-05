//! Multi-part audiobook attachments and the ebook side of the pair: the
//! parts survive a rescan as Unchanged, removing one sibling keeps the
//! rest, and removing or re-parsing the ebook leaves the attached
//! audiobook intact.

use super::{seed_audiobook, seed_ebook};
use crate::books::list_merged_rows_for_formats;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows as count, indexed, indexed_audiobook};

/// Merge an extra same-format `.m4b` part onto `book_id` the way a manual
/// multi-part merge does: forge the per-file ledger row, then replay the file
/// so the per-file attach accumulates it beside the existing parts.
async fn attach_extra_m4b_part(pool: &sqlx::SqlitePool, book_id: i64, group_path: &str) {
    let ab = indexed_audiobook(group_path, "Wind and Truth", Some("Brandon Sanderson"));
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, 'M4B', '/audio', ?)",
    )
    .bind(crate::helpers::stable_uuid("/audio", group_path))
    .bind(book_id)
    .bind(&ab.scan_key)
    .execute(pool)
    .await
    .unwrap();
    sync_audiobooks(
        pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

async fn seed_three_part_audiobook(pool: &sqlx::SqlitePool) -> i64 {
    seed_ebook(
        pool,
        "Sanderson/wt.epub",
        "Wind and Truth",
        "Brandon Sanderson",
    )
    .await;
    seed_audiobook(
        pool,
        "Sanderson/wt-1.m4b",
        "Wind and Truth",
        "Brandon Sanderson",
    )
    .await;
    let ebook_id: i64 = sqlx::query_scalar(
        "SELECT book_id FROM merged_uuids WHERE scan_key = 'Sanderson/wt-1.m4b'",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    attach_extra_m4b_part(pool, ebook_id, "Sanderson/wt-2.m4b").await;
    attach_extra_m4b_part(pool, ebook_id, "Sanderson/wt-3.m4b").await;
    ebook_id
}

#[tokio::test]
async fn multipart_audiobook_attachments_survive_rescan_as_unchanged() {
    // The #1126 core case: N same-format parts manually merged under one book.
    // Each keys on its own scan_key, so a rescan classifies every one Unchanged
    // instead of resurrecting all-but-one as standalone duplicates.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_three_part_audiobook(&pool).await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM merged_uuids WHERE format = 'M4B'"
        )
        .await,
        3
    );

    let disk: Vec<_> = [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
    ]
    .iter()
    .map(|gp| {
        let ab = indexed_audiobook(gp, "Wind and Truth", Some("Brandon Sanderson"));
        crate::ebook::StatEntry {
            filename: ab.group_path.clone(),
            scan_key: ab.scan_key.clone(),
            mtime_epoch: ab.max_mtime_epoch,
            size_bytes: ab.total_size_bytes,
            error: None,
        }
    })
    .collect();
    let db_rows = list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    assert_eq!(diff.unchanged.len(), 3);
    assert!(
        diff.new.is_empty(),
        "no part resurrects as a standalone book"
    );
    assert!(diff.removed.is_empty());
}

#[tokio::test]
async fn removing_one_multipart_sibling_keeps_the_other_parts() {
    // AC3: removing one part's file drops only that part's row — the old
    // `(book_id, format)` join would have deleted every M4B part at once.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_three_part_audiobook(&pool).await;

    let part2_uuid: String =
        sqlx::query_scalar("SELECT uuid FROM merged_uuids WHERE scan_key = 'Sanderson/wt-2.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            removed_uuids: vec![part2_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM merged_uuids WHERE format = 'M4B'"
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE scan_key = 'Sanderson/wt-2.m4b'"
        )
        .await,
        0
    );
}

#[tokio::test]
async fn removed_ebook_leaves_attached_audiobook_intact() {
    // The ebook's own book_files row is dropped when its file goes missing,
    // but the cross-format M4B — recorded in merged_uuids — is a different
    // format's file, still present on disk, and must survive rather than
    // being dropped and re-attached on a later scan (AC5). The old blanket
    // `book_id IN (...)` delete used to take both rows.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    // One book (the ebook), with the m4b attached via merged_uuids.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);

    // Identity is minted (F2) — remove by the real uuid (read via scan_key).
    let ebook_uuid = crate::test_support::uuid_by_scan_key(&pool, "Stoker/Dracula.epub").await;
    sync_books(
        &pool,
        "/ebooks",
        SyncPlan {
            removed_uuids: vec![ebook_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // The books row and its merged_uuids ledger survive; only the ebook's own
    // file row is gone — the attached M4B is untouched, no re-attach needed.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1
    );
    // The surviving M4B means the book isn't actually fileless — it must not
    // be flagged missing (the flag UPDATE is guarded on `NOT EXISTS
    // book_files`, so a surviving cross-format attachment excludes it).
    assert_eq!(
        count(&pool, "SELECT is_missing_files FROM books").await,
        0,
        "a book with a surviving cross-format attachment is not flagged missing"
    );
}

#[tokio::test]
async fn ebook_changed_reparse_preserves_attached_audiobook_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    sync_books(
        &pool,
        "/ebooks",
        SyncPlan {
            changed_books: vec![indexed(
                "Stoker/Dracula.epub",
                Some("Dracula (Annotated)"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // The format-scoped wipe must leave the attached M4B row (and its
    // parts) alone.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        1
    );
}
