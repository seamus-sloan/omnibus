//! The Moved bucket for audiobook groups: relocation in place (no
//! `merged_uuids` row), every part filename rewritten for multi-part and
//! single-file groups, convergence across repeated reindexes, and an
//! extension retype that keeps the uuid.

use sqlx::SqlitePool;

use super::super::shared::insert_new_audiobook;
use super::super::{sync_audiobooks, AudiobookSyncPlan};
use super::{book_files_count, seed_audiobook_with_file, seed_scan_root};
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed_audiobook, CoversTempDir};

/// AC6: AC1-AC4 hold for an audiobook group directory moved within the
/// audiobook library. The moved group is classified Removed (old scan_key)
/// and New (new scan_key) in the same reindex plan; the relocation must be
/// recognized and rewritten in place — updating `books.scan_key` (AC1),
/// preserving `books.id`/`books.uuid` (AC4), clearing `is_missing_files`
/// (AC2), and minting no `merged_uuids` row (AC3) — instead of being
/// re-bound as a cross-format attachment onto its own just-vacated slot.
#[tokio::test]
async fn moved_audiobook_group_relocates_in_place_instead_of_minting_a_merged_uuids_row() {
    let _covers = CoversTempDir::new("ab_sync_relocation");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Old/Book").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // One reindex pass observing the move: the old scan_key is Removed, the
    // new group directory is New — exactly what the real diff produces.
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("New/Book", "Seeded", Some("Seed Author"))],
            removed_uuids: vec![uuid_before.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no duplicate book was created"
    );
    let (id_after, uuid_after, scan_key_after, is_missing): (i64, String, String, i64) =
        sqlx::query_as("SELECT id, uuid, scan_key, is_missing_files FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "AC4: books.id preserved");
    assert_eq!(uuid_after, uuid_before, "AC4: books.uuid preserved");
    assert_eq!(scan_key_after, "New/Book", "AC1: scan_key follows the move");
    assert_eq!(
        is_missing, 0,
        "AC2: not flagged missing after the relocation"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "AC3: no cross-format ledger row minted for a same-format relocation"
    );
    assert_eq!(book_files_count(&pool, book_id).await, 1, "one file row");
}

/// Ordered `book_file_parts.filename` values under one book.
async fn part_filenames(pool: &SqlitePool, book_id: i64) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT p.filename FROM book_file_parts p
           JOIN book_files bf ON bf.id = p.book_file_id
          WHERE bf.book_id = ?
          ORDER BY p.ordinal",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await
    .unwrap()
}

/// A three-part MP3 group: every part filename is library-relative and
/// sits under the group directory, so a move has to rewrite all three.
fn multipart_mp3_group(group_path: &str) -> crate::audiobook::IndexedAudiobook {
    let mut b = indexed_audiobook(group_path, "Seeded", Some("Seed Author"));
    b.format = "MP3".into();
    b.parts = (0..3)
        .map(|i| crate::audiobook::AudiobookPart {
            ordinal: i,
            filename: format!("{group_path}/0{}.mp3", i + 1),
            size_bytes: 1000,
            mtime_epoch: 100,
            duration_seconds: 1200.0,
        })
        .collect();
    b.total_size_bytes = 3000;
    b
}

/// AC9: a moved audiobook group directory relocates every path column,
/// including each `book_file_parts.filename` — the HLS and download read
/// paths resolve part filenames against the audio root, so a stale part
/// path is a broken book even though `books` looks correct.
#[tokio::test]
async fn moved_audiobook_group_rewrites_every_part_filename() {
    let _covers = CoversTempDir::new("ab_moved_parts");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(
        &mut tx,
        library_id,
        &multipart_mp3_group("Old/Book"),
        &mut covers,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let (book_id, uuid): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Old/Book'")
            .fetch_one(&pool)
            .await
            .unwrap();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: uuid.clone(),
                filename: "New/Home/Book".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (scan_key, path, uuid_after): (String, String, String) =
        sqlx::query_as("SELECT scan_key, path, uuid FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(scan_key, "New/Home/Book", "AC9: books.scan_key moved");
    assert_eq!(path, "New/Home", "AC9: books.path is the group's parent");
    assert_eq!(uuid_after, uuid, "identity preserved");

    let (filename, file_scan_key): (String, String) =
        sqlx::query_as("SELECT filename, scan_key FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        filename, "Book",
        "book_files.filename is the group leaf stem"
    );
    assert_eq!(file_scan_key, "New/Home/Book");

    assert_eq!(
        part_filenames(&pool, book_id).await,
        vec![
            "New/Home/Book/01.mp3".to_string(),
            "New/Home/Book/02.mp3".to_string(),
            "New/Home/Book/03.mp3".to_string(),
        ],
        "AC9: every part path is rebased onto the new group directory"
    );
}

/// The single-file case: an `.m4b` group's one part *is* the group path,
/// so the rebase has to handle an empty suffix.
#[tokio::test]
async fn moved_single_file_audiobook_rewrites_its_one_part_path() {
    let _covers = CoversTempDir::new("ab_moved_single");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut b = indexed_audiobook("Old/Book.m4b", "Seeded", Some("Seed Author"));
    b.parts[0].filename = "Old/Book.m4b".into();
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    insert_new_audiobook(&mut tx, library_id, &b, &mut covers)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    let (book_id, uuid): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Old/Book.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();

    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid,
                filename: "New/Book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        part_filenames(&pool, book_id).await,
        vec!["New/Book.m4b".to_string()]
    );
    let filename: String = sqlx::query_scalar("SELECT filename FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(filename, "Book", "the extension is stripped for the stem");
}

/// AC9, second half: three consecutive reindexes after the move converge
/// with `is_missing_files = 0` throughout, driven by the real classifier
/// against a fixed on-disk state.
#[tokio::test]
async fn three_reindexes_after_a_moved_audiobook_group_converge_without_ghosting() {
    let _covers = CoversTempDir::new("ab_moved_convergence");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_audiobook_with_file(&pool, library_id, "Old/Book").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // What `project_groups_to_stat` hands the classifier for the moved
    // group: the group path as both filename and scan_key, carrying the
    // group's summed size and max mtime.
    let disk = [crate::ebook::StatEntry {
        filename: "New/Book".into(),
        scan_key: "New/Book".into(),
        mtime_epoch: 100,
        size_bytes: 1000,
        error: None,
    }];

    for pass in 0..3 {
        let db_rows = crate::books::list_indexed_rows_for_formats(&pool, "/lib", &["M4B"])
            .await
            .unwrap();
        let diff =
            crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
        if pass == 0 {
            assert_eq!(diff.moved.len(), 1, "pass 0: the group move is detected");
            assert!(diff.removed.is_empty(), "AC9: the group never ghosts");
            assert!(diff.new.is_empty(), "AC9: no Phase-B parse target");
        } else {
            assert!(diff.moved.is_empty(), "pass {pass}: already settled");
            assert_eq!(diff.unchanged.len(), 1, "pass {pass}: classifies Unchanged");
        }

        sync_audiobooks(
            &pool,
            "/lib",
            AudiobookSyncPlan {
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
        assert_eq!(scan_key, "New/Book", "pass {pass}: scan_key settled");
        assert_eq!(
            is_missing, 0,
            "AC9: is_missing_files stays 0 on pass {pass}"
        );
        assert_eq!(
            part_filenames(&pool, book_id).await,
            vec!["New/Book/part1.m4b".to_string()],
            "pass {pass}: the part path stays rebased"
        );
        assert_eq!(
            count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
            0,
            "pass {pass}: no ledger row minted"
        );
    }
}

/// End-to-end on the regression the format guard exists for: retyping
/// `Book.m4a` to `Book.m4b` preserves the bytes, so the stat pair matches
/// exactly — but `book_files.format` drives path resolution, and the move
/// writer rewrites path columns only. The classifier must therefore route
/// it Removed + New, where the Phase-B re-parse writes the *correct*
/// format and #1534's title+author relocation still preserves the uuid.
/// Asserting through `book_file_path` is the point: a stale format leaves a
/// book that resolves to a file which is not on disk.
#[tokio::test]
async fn retyping_an_audiobook_extension_reindexes_with_the_new_format_and_keeps_its_uuid() {
    let _covers = CoversTempDir::new("ab_retype_extension");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut seed = indexed_audiobook("Author/Book.m4a", "Book", Some("Seed Author"));
    seed.format = "M4A".into();
    seed.parts[0].filename = "Author/Book.m4a".into();
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![seed],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (book_id, uuid_before): (i64, String) =
        sqlx::query_as("SELECT id, uuid FROM books WHERE scan_key = 'Author/Book.m4a'")
            .fetch_one(&pool)
            .await
            .unwrap();

    // The same group, same bytes, now carrying the `.m4b` extension.
    let disk = [crate::ebook::StatEntry {
        filename: "Author/Book.m4b".into(),
        scan_key: "Author/Book.m4b".into(),
        mtime_epoch: 100,
        size_bytes: 1000,
        error: None,
    }];
    let db_rows = crate::books::list_indexed_rows_for_formats(
        &pool,
        "/lib",
        crate::audiobook::AUDIOBOOK_FORMATS,
    )
    .await
    .unwrap();
    let diff = crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
    assert!(diff.moved.is_empty(), "a format change is not a relocation");
    assert_eq!(diff.removed.len(), 1);
    assert_eq!(diff.new.len(), 1);

    let mut retyped = indexed_audiobook("Author/Book.m4b", "Book", Some("Seed Author"));
    retyped.format = "M4B".into();
    retyped.parts[0].filename = "Author/Book.m4b".into();
    sync_audiobooks(
        &pool,
        "/lib",
        AudiobookSyncPlan {
            new_books: vec![retyped],
            moved: diff.moved,
            removed_uuids: diff.removed,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "no duplicate book"
    );
    let (uuid_after, scan_key, format): (String, String, String) = sqlx::query_as(
        "SELECT b.uuid, b.scan_key, bf.format FROM books b
           JOIN book_files bf ON bf.book_id = b.id WHERE b.id = ?",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(uuid_after, uuid_before, "identity survives the retype");
    assert_eq!(scan_key, "Author/Book.m4b");
    assert_eq!(format, "M4B", "the re-parse wrote the new format");
    assert_eq!(
        crate::books::book_file_path(&pool, book_id, "M4B")
            .await
            .unwrap(),
        Some(std::path::PathBuf::from("/lib/Author/Book.m4b")),
        "the book resolves to the file that is actually on disk"
    );
    assert_eq!(
        crate::books::book_file_path(&pool, book_id, "M4A")
            .await
            .unwrap(),
        None,
        "no row is left claiming the old extension"
    );
}
