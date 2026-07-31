//! Tests for cross-format book attachment: matching a new audiobook/ebook
//! to an existing book of the other format by normalized title + author
//! (including last/first name order), and the ambiguity/ownership guards
//! that skip a match rather than clobbering an existing attachment.

use crate::books::list_merged_rows_for_formats;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows as count, indexed, indexed_audiobook, CoversTempDir};

async fn seed_ebook(pool: &sqlx::SqlitePool, filename: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_ebook(pool, filename, title, author).await;
}

async fn seed_audiobook(pool: &sqlx::SqlitePool, group_path: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_audiobook(pool, group_path, title, author).await;
}

#[tokio::test]
async fn audiobook_attaches_to_existing_ebook_with_matching_title_and_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    let (mu_book, mu_lib): (i64, String) =
        sqlx::query_as("SELECT book_id, library_path FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let ebook_id: i64 = sqlx::query_scalar("SELECT id FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(mu_book, ebook_id);
    assert_eq!(mu_lib, "/audio");
    // The attached file row carries its own location override so HLS
    // resolves parts against the audio root, not the ebook library.
    let (bf_lib, bf_path): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT library_path, path FROM book_files WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bf_lib.as_deref(), Some("/audio"));
    assert_eq!(bf_path.as_deref(), Some("Stoker"));
    // Parts and chapters landed under the attached file row.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        1
    );
    // Target metadata untouched: the ebook's title/author survive.
    let title: String = sqlx::query_scalar("SELECT title FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Dracula");
}

#[tokio::test]
async fn ebook_attaches_to_existing_audiobook_with_matching_title_and_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    let mu_lib: String =
        sqlx::query_scalar("SELECT library_path FROM merged_uuids WHERE format = 'EPUB'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mu_lib, "/ebooks");
}

#[tokio::test]
async fn attach_matches_author_across_last_first_name_order() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Stoker, Bram").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn attach_skipped_when_target_already_has_the_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    // Second M4B of the same work (e.g. a different rip) stays separate.
    seed_audiobook(&pool, "B/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
}

#[tokio::test]
async fn attach_skipped_when_title_is_ambiguous_across_candidates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Two same-format copies of the work = two candidate books.
    seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_ebook(&pool, "B/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 3);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
}

#[tokio::test]
async fn attach_skipped_when_book_has_no_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("Stoker/Dracula.m4b", "Dracula", None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
}

#[tokio::test]
async fn attach_skipped_when_titles_differ() {
    // The whole point of exact matching: Dune must not absorb Dune Messiah.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Herbert/Dune.epub", "Dune", "Frank Herbert").await;
    seed_audiobook(
        &pool,
        "Herbert/Dune Messiah.m4b",
        "Dune Messiah",
        "Frank Herbert",
    )
    .await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
}

#[tokio::test]
async fn two_audiobooks_matching_one_ebook_in_one_plan_do_not_clobber() {
    // Two distinct M4B files for the same work land in a single New plan. The
    // first attaches as the ebook's M4B edition; the second must become its
    // own book, not overwrite the first's file row.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(
        &pool,
        "Sanderson/Wind and Truth.epub",
        "Wind and Truth",
        "Brandon Sanderson",
    )
    .await;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![
                indexed_audiobook(
                    "Sanderson/wt-a.m4b",
                    "Wind and Truth",
                    Some("Brandon Sanderson"),
                ),
                indexed_audiobook(
                    "Sanderson/wt-b.m4b",
                    "Wind and Truth",
                    Some("Brandon Sanderson"),
                ),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // One attaches to the ebook, the other stands alone → 2 books, and both
    // M4B files survive (no delete-then-replace).
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        2
    );
    // Only the attached file records a ledger row; the standalone book is
    // native, so exactly one merged_uuids row exists.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);
}

#[tokio::test]
async fn replayed_second_file_accumulates_as_another_part_under_the_book() {
    // Two merged_uuids rows pointing at the same (book, M4B) — a deliberate
    // multi-part `.m4b` merge (#1126). Attachments key on the file's own
    // scan_key, so replaying the second file accumulates it as another part
    // beside the first rather than clobbering it or resurrecting a standalone
    // book. (Pre-#1126 the second file was demoted to its own book.)
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(
        &pool,
        "Sanderson/Wind and Truth.epub",
        "Wind and Truth",
        "Brandon Sanderson",
    )
    .await;

    // File A attaches to the ebook the normal way.
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "Sanderson/wt-a.m4b",
                "Wind and Truth",
                Some("Brandon Sanderson"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let ebook_id: i64 = sqlx::query_scalar(
        "SELECT book_id FROM merged_uuids WHERE scan_key = 'Sanderson/wt-a.m4b'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    // Forge the legacy bad state: a second ledger row for file B on A's slot.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('forged-b', ?, 'M4B', '/audio', 'Sanderson/wt-b.m4b')",
    )
    .bind(ebook_id)
    .execute(&pool)
    .await
    .unwrap();

    // Replay file B through the Changed bucket.
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            changed_books: vec![indexed_audiobook(
                "Sanderson/wt-b.m4b",
                "Wind and Truth",
                Some("Brandon Sanderson"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // The incumbent A's file row survives — the scoped delete only touches B's
    // own scan_key — and B is now a second M4B row under the *same* ebook.
    let stems: Vec<String> = sqlx::query_scalar(
        "SELECT filename FROM book_files WHERE book_id = ? AND format = 'M4B' ORDER BY ordinal",
    )
    .bind(ebook_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(stems, vec!["wt-a".to_string(), "wt-b".to_string()]);
    // Both parts hang off the ebook — no standalone book was resurrected for B.
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM book_files WHERE book_id = {ebook_id} AND format = 'M4B'"
            )
        )
        .await,
        2
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        2
    );
    // Both parts keep their guard rows, each keyed on its own scan_key.
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM merged_uuids WHERE book_id = {ebook_id} AND format = 'M4B'"
            )
        )
        .await,
        2
    );
}

#[tokio::test]
async fn replayed_ebook_ledger_collision_does_not_clobber_the_incumbent_file() {
    // The ebook attach writer shares the same guard. Same shape as the
    // audiobook case, roles swapped: a native audiobook holds an attached EPUB,
    // and a forged second-EPUB ledger row must not clobber the incumbent.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_audiobook(&pool, "Herbert/Dune.m4b", "Dune", "Frank Herbert").await;

    // EPUB A attaches to the audiobook.
    sync_books(
        &pool,
        "/books",
        SyncPlan {
            new_books: vec![indexed(
                "Herbert/dune-a.epub",
                Some("Dune"),
                &["Frank Herbert"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT book_id FROM merged_uuids WHERE format = 'EPUB'")
        .fetch_one(&pool)
        .await
        .unwrap();

    // Forge a second EPUB ledger row for file B on A's slot.
    let scan_key_b = crate::helpers::scan_key_for("Herbert/dune-b.epub");
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('forged-eb', ?, 'EPUB', '/books', ?)",
    )
    .bind(book_id)
    .bind(&scan_key_b)
    .execute(&pool)
    .await
    .unwrap();

    // Replay EPUB B through the Changed bucket.
    sync_books(
        &pool,
        "/books",
        SyncPlan {
            changed_books: vec![indexed(
                "Herbert/dune-b.epub",
                Some("Dune"),
                &["Frank Herbert"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Incumbent A's EPUB row is intact; B became its own book.
    let incumbent: String =
        sqlx::query_scalar("SELECT filename FROM book_files WHERE book_id = ? AND format = 'EPUB'")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(incumbent, "dune-a");
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        2
    );
}

#[tokio::test]
async fn reindex_diff_classifies_attached_file_as_unchanged() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // The next audiobook reindex feeds merged rows into the diff; a
    // matching on-disk stat must classify Unchanged, not New.
    let ab = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let db_rows = list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        scan_key: ab.scan_key.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    // The merged row's identity is `merged_uuids.uuid` = the attach ledger
    // key (stable_uuid of the audiobook's group path), unchanged by F2.
    assert_eq!(
        diff.unchanged,
        vec![crate::helpers::stable_uuid("/audio", &ab.group_path)]
    );
    assert!(diff.new.is_empty());
    assert!(diff.removed.is_empty());
}

#[tokio::test]
async fn changed_attached_file_refreshes_file_row_without_touching_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // Re-sync the audiobook through the Changed bucket with a new stat
    // and a (suspicious) new title — the file row must refresh, the
    // target book's metadata must not.
    let mut ab = indexed_audiobook(
        "Stoker/Dracula.m4b",
        "Dracula Unabridged",
        Some("Bram Stoker"),
    );
    ab.total_size_bytes = 9999;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            changed_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    let size: i64 = sqlx::query_scalar("SELECT size_bytes FROM book_files WHERE format = 'M4B'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(size, 9999);
    let title: String = sqlx::query_scalar("SELECT title FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Dracula");
}

#[tokio::test]
async fn removed_attached_file_drops_attachment_but_keeps_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    let ab_uuid: String = sqlx::query_scalar("SELECT uuid FROM merged_uuids")
        .fetch_one(&pool)
        .await
        .unwrap();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            removed_uuids: vec![ab_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        0
    );
}

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

#[tokio::test]
async fn attached_audiobook_cover_is_adopted_when_target_has_none() {
    let _covers = CoversTempDir::new("attach_adopt");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;

    let mut ab = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    ab.cover = Some(("image/jpeg".into(), vec![1, 2, 3]));
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let has_cover: i64 = sqlx::query_scalar("SELECT has_cover FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(has_cover, 1);
}

#[tokio::test]
async fn known_uuid_reattaches_even_when_titles_no_longer_match() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // Simulate the self-healing path: the attachment row vanished but
    // merged_uuids still knows the file. A New sync with a *different*
    // title must still re-attach via the uuid.
    sqlx::query("DELETE FROM book_files WHERE format = 'M4B'")
        .execute(&pool)
        .await
        .unwrap();
    seed_audiobook(
        &pool,
        "Stoker/Dracula.m4b",
        "Dracula: Special Edition",
        "Bram Stoker",
    )
    .await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1
    );
}

/// The attach-ledger scan_key lookup (shared by `find_attachment_by_scan_key`
/// and `record_attachment`, fired once per file per reindex) must ride
/// `idx_merged_uuids_library_scan`, not scan. Guards against regressing to
/// the un-indexed form the table shipped with in migration 0026.
#[tokio::test]
async fn merged_uuids_library_scan_lookup_uses_index() {
    use sqlx::Row;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan: String = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT uuid, book_id, format FROM merged_uuids
          WHERE library_path = ? AND scan_key = ?",
    )
    .bind("/audio")
    .bind("Stoker/Dracula.m4b")
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join("\n");
    assert!(
        plan.contains("idx_merged_uuids_library_scan"),
        "scan_key lookup should use idx_merged_uuids_library_scan, got plan:\n{plan}"
    );
}

/// The multiformat listing query in `list_merged_rows_for_formats` must
/// ride `idx_merged_uuids_library_format`, not scan the whole ledger.
#[tokio::test]
async fn merged_uuids_library_format_listing_uses_index() {
    use sqlx::Row;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan: String = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT mu.uuid FROM merged_uuids mu
          WHERE mu.library_path = ? AND mu.format IN (?, ?, ?)",
    )
    .bind("/audio")
    .bind("M4B")
    .bind("M4A")
    .bind("MP3")
    .fetch_all(&pool)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("detail"))
    .collect::<Vec<_>>()
    .join("\n");
    assert!(
        plan.contains("idx_merged_uuids_library_format"),
        "format listing should use idx_merged_uuids_library_format, got plan:\n{plan}"
    );
}

#[tokio::test]
async fn attachment_survives_a_scan_root_repoint() {
    // F2: an attached file is matched by its repoint-stable
    // `(library_path, scan_key)`, and a repoint updates
    // `merged_uuids.library_path`, so re-scanning under the new root
    // re-attaches against the stored ledger uuid instead of duplicating.
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::settings::set_settings(
        &pool,
        &omnibus_shared::Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: Some("/audio".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);

    // Repoint the audiobook scan root /audio -> /audio2.
    crate::settings::set_settings(
        &pool,
        &omnibus_shared::Settings {
            ebook_library_path: Some("/ebooks".into()),
            audiobook_library_path: Some("/audio2".into()),
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let mu_lib: String =
        sqlx::query_scalar("SELECT library_path FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        mu_lib, "/audio2",
        "the attach ledger's library_path follows the repoint"
    );

    // Re-scan the same m4b under the new root: it must re-attach, not duplicate.
    sync_audiobooks(
        &pool,
        "/audio2",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "Stoker/Dracula.m4b",
                "Dracula",
                Some("Bram Stoker"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "re-scan under the new root re-attaches — no duplicate book"
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        1,
        "no duplicate ledger row"
    );
}
