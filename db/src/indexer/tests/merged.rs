//! Reindexing books whose files were merged into one record: each file is
//! classified independently, repeated scans stay `Unchanged`, and a whole
//! library reorganization is recovered as moves rather than delete + add.

use crate::books::{list_indexed_rows_for_formats, list_merged_rows_for_formats, IndexedRow};
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{
    count_rows, indexed, indexed_audiobook, indexed_with_stat, make_test_dir, uuid_by_scan_key,
    CoversTempDir,
};

use super::super::*;
use super::{entry, seed_ebook_at};

// ---------- #1537: a multi-file book is diffed per file, not per book ----------

/// Seed two EPUBs through the real `sync_books` write path with distinct
/// on-disk stats, then merge the second into the first — the exact repro
/// shape from #1537 (a book left holding two same-format `book_files`
/// rows). Returns `(target_uuid, source_uuid)`.
async fn seed_merged_two_epub_book(pool: &SqlitePool) -> (String, String) {
    sync_books(
        pool,
        "/ebooks",
        SyncPlan {
            new_books: vec![indexed_with_stat("A/one.epub", Some("One"), 1000, 500)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let target = uuid_by_scan_key(pool, "A/one.epub").await;

    sync_books(
        pool,
        "/ebooks",
        SyncPlan {
            new_books: vec![indexed_with_stat("B/two.epub", Some("Two"), 2000, 100)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = uuid_by_scan_key(pool, "B/two.epub").await;

    crate::merge::merge_books(pool, &source, &target, None)
        .await
        .unwrap();
    (target, source)
}

/// Build the DB side of the diff exactly as `reindex` does for the ebook
/// pass: the anchor row(s) via `list_indexed_rows_for_formats`, plus every
/// attached file via `list_merged_rows_for_formats`.
async fn ebook_db_rows(pool: &SqlitePool, library_path: &str) -> Vec<IndexedRow> {
    let mut rows = list_indexed_rows_for_formats(pool, library_path, crate::ebook::EBOOK_FORMATS)
        .await
        .unwrap();
    rows.extend(
        list_merged_rows_for_formats(pool, library_path, crate::ebook::EBOOK_FORMATS)
            .await
            .unwrap(),
    );
    rows
}

#[tokio::test]
async fn multi_file_book_from_merge_classifies_each_file_independently_when_one_changes() {
    // AC3: modifying only the second file's stat must classify that file
    // Changed while the first stays Unchanged, and vice versa. Before the
    // fix, `list_indexed_rows_for_formats` aggregated MAX(mtime)/MAX(size)
    // across both `book_files` rows, so this book reclassified Changed
    // regardless of which file (or neither) actually moved.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (target, source) = seed_merged_two_epub_book(&pool).await;
    let db_rows = ebook_db_rows(&pool, "/ebooks").await;

    // Only the second file (`B/two.epub`) changed on disk.
    let disk_second_changed = vec![
        entry("A/one.epub", "A/one.epub", 1000, 500),
        entry("B/two.epub", "B/two.epub", 2000, 999),
    ];
    let diff = diff_library(&disk_second_changed, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(diff.unchanged, vec![target.clone()], "{diff:?}");
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].filename, "B/two.epub");

    // Only the first file (`A/one.epub`) changed on disk.
    let disk_first_changed = vec![
        entry("A/one.epub", "A/one.epub", 1000, 999),
        entry("B/two.epub", "B/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk_first_changed, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(diff.unchanged, vec![source], "{diff:?}");
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].filename, "A/one.epub");
}

/// AC1 + AC2: a book merged from two same-format EPUBs classifies both
/// files Unchanged on a rescan where neither changed, and stays that way
/// across three consecutive full reindexes — no re-parse, so each file's
/// `book_files.id` (and the book's `last_modified`) survives unchanged.
/// A Changed rewrite always mints a fresh `book_files` row (DELETE +
/// INSERT), so a stable id is direct evidence no re-parse happened.
#[tokio::test]
async fn merged_two_file_book_stays_unchanged_across_three_consecutive_reindexes() {
    let _covers = CoversTempDir::new("merge-reparse-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("merge-reparse-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "A/one.epub", "One").await;
    seed_ebook_at(&pool, &lib_path, "B/two.epub", "Two").await;
    let target = uuid_by_scan_key(&pool, "A/one.epub").await;
    let source = uuid_by_scan_key(&pool, "B/two.epub").await;
    crate::merge::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let file_ids_before: Vec<i64> = sqlx::query_scalar("SELECT id FROM book_files ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        file_ids_before.len(),
        2,
        "test precondition: two files merged"
    );
    let last_modified_before: i64 =
        sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
            .bind(&target)
            .fetch_one(&pool)
            .await
            .unwrap();

    for pass in 1..=3 {
        reindex(&pool, &lib_path).await.unwrap();
        let file_ids_after: Vec<i64> = sqlx::query_scalar("SELECT id FROM book_files ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            file_ids_after, file_ids_before,
            "reindex pass {pass} re-wrote a book_files row (re-parsed) \
             instead of classifying Unchanged"
        );
        let last_modified_after: i64 =
            sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
                .bind(&target)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            last_modified_after, last_modified_before,
            "reindex pass {pass} touched books.last_modified (implies a Changed rewrite)"
        );
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC6: an audiobook naturally holding two different-format groups under
/// one book (auto-attached by title+author match — not a manual merge)
/// classifies both groups Unchanged on a rescan where neither changed.
#[tokio::test]
async fn mixed_format_audiobook_group_classifies_both_parts_unchanged_when_nothing_changed() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut m4b = indexed_audiobook("A/Dracula.m4b", "Dracula", Some("Stoker"));
    m4b.max_mtime_epoch = 1000;
    m4b.total_size_bytes = 500;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![m4b],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let target = uuid_by_scan_key(&pool, "A/Dracula.m4b").await;

    // Same title+author, different format — auto-attaches to the M4B book
    // instead of minting a second `books` row.
    let mut mp3 = indexed_audiobook("B/Dracula mp3", "Dracula", Some("Stoker"));
    mp3.format = "MP3".into();
    mp3.max_mtime_epoch = 2000;
    mp3.total_size_bytes = 100;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "test precondition: the MP3 group auto-attached rather than minting a new book"
    );
    let source = crate::test_support::uuid_by_scan_key(&pool, "B/Dracula mp3").await;
    assert_ne!(
        target, source,
        "the attached group keeps its own ledger uuid"
    );

    let disk = vec![
        entry("A/Dracula.m4b", "A/Dracula.m4b", 1000, 500),
        entry("B/Dracula mp3", "B/Dracula mp3", 2000, 100),
    ];
    let mut db_rows =
        list_indexed_rows_for_formats(&pool, "/audio", crate::audiobook::AUDIOBOOK_FORMATS)
            .await
            .unwrap();
    db_rows.extend(
        list_merged_rows_for_formats(&pool, "/audio", crate::audiobook::AUDIOBOOK_FORMATS)
            .await
            .unwrap(),
    );

    let diff = diff_library(&disk, &db_rows, Path::new("/audio"), true);
    assert_eq!(diff.unchanged.len(), 2, "{diff:?}");
    assert!(diff.unchanged.contains(&target));
    assert!(diff.unchanged.contains(&source));
    assert!(diff.new.is_empty(), "{:?}", diff.new);
    assert!(diff.changed.is_empty(), "{:?}", diff.changed);
}

// ---------- #1536: relocation, end-to-end through `reindex` ----------

/// Index one stub EPUB of a caller-chosen byte length at `library_path`,
/// seeding the row with the file's real stat so a later `reindex`
/// classifies it Unchanged on a healthy pass. Distinct lengths give each
/// book a distinct `(size, mtime)` pair — the relocation detector's primary
/// uniqueness path, rather than its stem tiebreaker.
async fn seed_sized_ebook_at(pool: &SqlitePool, library_path: &str, filename: &str, len: usize) {
    let abs = std::path::Path::new(library_path).join(filename);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, vec![b'x'; len]).unwrap();
    let meta = std::fs::metadata(&abs).unwrap();
    let mut book = indexed(filename, Some("Title"), &["Author"], &[], None, None);
    book.mtime_epoch = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    book.size_bytes = meta.len() as i64;
    sync_books(
        pool,
        library_path,
        SyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

/// AC3, end-to-end: reorganizing a whole library on disk — far past both
/// `MASS_MISSING_MIN_ABSOLUTE` and `MASS_MISSING_FRACTION` — no longer
/// aborts the reindex, because every file lands in Moved instead of
/// Removed. Before this detector the same scan was a hard
/// `MassMissingError`, which is why the title+author path never got the
/// chance to recover it.
#[tokio::test]
async fn reindex_recovers_a_whole_library_reorganization_without_tripping_the_breaker() {
    let _covers = CoversTempDir::new("reindex-reorg");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-reorg-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    // Enough books to clear the absolute floor, so the percentage guard is
    // actually reachable — below it the scan would be waved through for a
    // reason that has nothing to do with move detection, and the test would
    // prove nothing. A `const` block so the precondition is a compile error.
    const BOOKS: usize = 12;
    const { assert!(BOOKS > MASS_MISSING_MIN_ABSOLUTE) };
    for i in 0..BOOKS {
        seed_sized_ebook_at(&pool, &lib_path, &format!("Old/{i}.epub"), 100 + i * 7).await;
    }
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT scan_key, uuid FROM books ORDER BY scan_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(before.len(), BOOKS);

    // Reorganize every file into a different directory, preserving bytes
    // and timestamps — what `mv` does within one filesystem.
    std::fs::create_dir_all(lib.join("New")).unwrap();
    for i in 0..BOOKS {
        std::fs::rename(
            lib.join(format!("Old/{i}.epub")),
            lib.join(format!("New/{i}.epub")),
        )
        .unwrap();
    }
    std::fs::remove_dir(lib.join("Old")).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.moved, BOOKS, "AC3: every file was matched as moved");
    assert_eq!(
        stats.removed, 0,
        "AC3: nothing ghosted, so nothing to abort"
    );
    assert_eq!(stats.ghost_warning(), None, "a move is not a ghost");

    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT scan_key, uuid FROM books ORDER BY scan_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(after.len(), BOOKS, "no duplicates were inserted");
    for ((old_key, old_uuid), (new_key, new_uuid)) in before.iter().zip(after.iter()) {
        assert_eq!(old_key.replace("Old/", "New/"), *new_key);
        assert_eq!(old_uuid, new_uuid, "identity rode along with the move");
    }
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "AC1: no ledger rows minted for same-format relocations"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books WHERE is_missing_files = 1"
        )
        .await,
        0,
        "no book was flagged missing"
    );
    // Every file row followed its book.
    let stale: i64 = count_rows(
        &pool,
        "SELECT COUNT(*) FROM book_files WHERE scan_key NOT LIKE 'New/%'",
    )
    .await;
    assert_eq!(
        stale, 0,
        "AC2: every book_files.scan_key names the new path"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// The same library, with the same reorganization, but one file's bytes
/// changed on the way: it falls out of Moved into Removed + New (AC6)
/// while its neighbours still relocate.
#[tokio::test]
async fn reindex_leaves_a_relocation_that_also_changed_bytes_in_removed_and_new() {
    let _covers = CoversTempDir::new("reindex-reorg-edited");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-reorg-edited-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    for i in 0..3 {
        seed_sized_ebook_at(&pool, &lib_path, &format!("Old/{i}.epub"), 100 + i * 7).await;
    }
    std::fs::create_dir_all(lib.join("New")).unwrap();
    for i in 0..3 {
        std::fs::rename(
            lib.join(format!("Old/{i}.epub")),
            lib.join(format!("New/{i}.epub")),
        )
        .unwrap();
    }
    // Rewrite one file at a different length — its stat pair no longer
    // matches the row it came from.
    std::fs::write(lib.join("New/1.epub"), vec![b'y'; 4096]).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.moved, 2, "AC6: only the untouched files relocated");
    assert_eq!(stats.removed, 1, "AC6: the edited file's old row ghosted");

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC8, at the classifier: for a book holding two EPUBs (via
/// `merge_books`), moving **either** file is detected independently.
/// Candidacy is deliberately not gated on the book's file count — that
/// would silently exclude the anchor file of every merged book, which is
/// exactly the case this detector most needs to handle.
#[tokio::test]
async fn each_file_of_a_merged_two_epub_book_is_independently_move_matched() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (target, source) = seed_merged_two_epub_book(&pool).await;
    let db_rows = ebook_db_rows(&pool, "/ebooks").await;

    // The anchor file — the one `books.scan_key` names — moved.
    let disk = vec![
        entry("Moved/one.epub", "Moved/one.epub", 1000, 500),
        entry("B/two.epub", "B/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(
        diff.moved,
        vec![crate::sync::MovedFile {
            uuid: target.clone(),
            filename: "Moved/one.epub".into(),
        }],
        "the anchor of a merged book is move-matched by its own books.uuid: {diff:?}"
    );
    assert_eq!(
        diff.unchanged,
        vec![source.clone()],
        "the sibling is untouched"
    );
    assert!(diff.removed.is_empty());
    assert!(diff.new.is_empty());

    // The merged-in file moved instead — matched by its ledger uuid.
    let disk = vec![
        entry("A/one.epub", "A/one.epub", 1000, 500),
        entry("Moved/two.epub", "Moved/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(
        diff.moved,
        vec![crate::sync::MovedFile {
            uuid: source.clone(),
            filename: "Moved/two.epub".into(),
        }],
        "the merged-in file is move-matched by its merged_uuids handle: {diff:?}"
    );
    assert_eq!(diff.unchanged, vec![target.clone()]);

    // Both moved in one scan — their stat pairs differ, so both land.
    let disk = vec![
        entry("Moved/one.epub", "Moved/one.epub", 1000, 500),
        entry("Moved/two.epub", "Moved/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    let mut moved: Vec<(String, String)> = diff
        .moved
        .iter()
        .map(|m| (m.uuid.clone(), m.filename.clone()))
        .collect();
    moved.sort();
    let mut expected = vec![
        (target, "Moved/one.epub".to_string()),
        (source, "Moved/two.epub".to_string()),
    ];
    expected.sort();
    assert_eq!(moved, expected);
    assert!(diff.removed.is_empty());
    assert!(diff.new.is_empty());
}
