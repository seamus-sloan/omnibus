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
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"));
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

#[tokio::test]
async fn removed_ebook_goes_fileless_and_audiobook_reattaches() {
    // F2: removing the ebook file makes its books row fileless (retained, fileless)
    // rather than deleting it, so the attachment ledger survives. The book's
    // identity (and any user data on it) is preserved, and re-scanning the
    // still-present audiobook re-attaches to the same row — now audiobook-only.
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
    // Fileless: the books row + its merged_uuids ledger survive; all file rows
    // are dropped.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 0);

    // The audiobook is still on disk; the next scan re-attaches it to the
    // retained row, which becomes file-backed (audiobook-only) again.
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
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

/// The reindex-hot lookup keyed on `(library_path, scan_key)` must seek the
/// composite index, not scan the ledger. Guards migration 0035.
#[tokio::test]
async fn find_attachment_by_scan_key_uses_library_scan_index() {
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
        "attach lookup should use idx_merged_uuids_library_scan, got plan:\n{plan}"
    );
}

/// The landing/browse projection filtering merged rows by
/// `(library_path, format)` must seek the composite index, not scan.
/// Guards migration 0035.
#[tokio::test]
async fn list_merged_rows_for_formats_uses_library_format_index() {
    use sqlx::Row;
    let pool = init_db("sqlite::memory:").await.unwrap();
    let plan: String = sqlx::query(
        "EXPLAIN QUERY PLAN
         SELECT mu.uuid
           FROM merged_uuids mu
           JOIN book_files bf ON bf.book_id = mu.book_id AND bf.format = mu.format
          WHERE mu.library_path = ?
            AND mu.format IN (?, ?, ?)",
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
        "merged-rows projection should use idx_merged_uuids_library_format, got plan:\n{plan}"
    );
}

/// End-to-end confirmation that the new indexes are non-destructive: the
/// attach ledger still returns the expected row after the index migration.
/// Calls `super::find_attachment_by_scan_key` directly (rather than a raw
/// `SELECT`) so any future query change in the helper is caught by the test.
#[tokio::test]
async fn find_attachment_by_scan_key_returns_expected_row_after_indexes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    let mut tx = pool.begin().await.unwrap();
    let hit = super::find_attachment_by_scan_key(&mut tx, "/audio", "Stoker/Dracula.m4b")
        .await
        .unwrap()
        .expect("attach ledger row for the seeded audiobook must exist");
    assert_eq!(hit.2, "M4B", "format column should match the audiobook seed");
}
