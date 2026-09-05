//! The Moved bucket for a single-file ebook: relocation in place (no
//! `merged_uuids` row), every path column repointed with identity and
//! user data kept, `last_modified` untouched, convergence across repeated
//! reindexes, and the skip paths for a taken scan key or an unresolvable
//! uuid.

use sqlx::SqlitePool;

use super::super::{sync_books, SyncPlan};
use super::{book_files_count, book_with_all_links, seed_book_with_file, seed_scan_root};
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{count_rows, CoversTempDir};

/// A moved/renamed ebook is classified Removed (old scan_key) and New (new
/// scan_key) in the same reindex plan. The relocation must be recognized and
/// written as the book's own native row — updating `books.scan_key` (AC1),
/// preserving `books.id`/`books.uuid` (AC4), clearing `is_missing_files`
/// (AC2), and minting no `merged_uuids` row (AC3) — rather than the
/// title+author attach heuristic re-binding it as a cross-format attachment
/// onto its own just-vacated slot.
#[tokio::test]
async fn moved_ebook_relocates_in_place_instead_of_minting_a_merged_uuids_row() {
    let _covers = CoversTempDir::new("sync_books_relocation");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // One reindex pass observing the move: the old scan_key is Removed, the
    // new path is New — exactly what the real diff produces.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![book_with_all_links("New/book.epub", "Seeded")],
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
    assert_eq!(
        scan_key_after, "New/book.epub",
        "AC1: scan_key follows the move"
    );
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

/// AC2: three consecutive reindexes after a move converge. Each pass derives
/// its plan the way the real indexer does — `diff_library` classifying the
/// current DB rows against a fixed on-disk state — so a bug that leaves
/// `books.scan_key` stale (still naming the old path) would keep
/// reclassifying the book Removed every pass, flapping it between
/// present and missing forever.
#[tokio::test]
async fn three_consecutive_reindexes_after_a_move_converge_on_one_file_backed_row() {
    let _covers = CoversTempDir::new("sync_books_relocation_convergence");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;

    let disk = [crate::ebook::StatEntry {
        filename: "New/book.epub".into(),
        scan_key: "New/book.epub".into(),
        mtime_epoch: 0,
        size_bytes: 0,
        error: None,
    }];

    for pass in 0..3 {
        let db_rows = crate::books::list_indexed_rows_for_formats(&pool, "/lib", &["EPUB"])
            .await
            .unwrap();
        let diff = diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
        let new_books = if diff.new.is_empty() {
            vec![]
        } else {
            vec![book_with_all_links("New/book.epub", "Seeded")]
        };

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                new_books,
                removed_uuids: diff.removed,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert_eq!(
            book_files_count(&pool, book_id).await,
            1,
            "pass {pass}: exactly one file row, no flapping"
        );
        let is_missing: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(is_missing, 0, "pass {pass}: is_missing_files stays 0");
        assert_eq!(
            count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
            0,
            "pass {pass}: still no merged_uuids row"
        );
    }
}

/// Seed one user and return its id — the owner of the soft-ref user-data
/// rows the relocation tests assert survive a move.
async fn seed_user(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash) VALUES ('reader', 'x') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Attach one row of every soft-ref user-data kind to `book_uuid`:
/// progress, rating, shelf membership, annotation, journal entry.
async fn seed_user_data(pool: &SqlitePool, user_id: i64, book_uuid: &str) {
    sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi)
         VALUES (?, ?, 'epub', 'epubcfi(/6/4!/4/2)')",
    )
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 8)")
        .bind(user_id)
        .bind(book_uuid)
        .execute(pool)
        .await
        .unwrap();
    let shelf_id: i64 = sqlx::query_scalar(
        "INSERT INTO shelves (owner_user_id, kind, name) VALUES (?, 'manual', 'Favourites')
         RETURNING id",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO shelf_books (shelf_id, book_uuid) VALUES (?, ?)")
        .bind(shelf_id)
        .bind(book_uuid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO annotations (user_id, book_uuid, epub_cfi_range, text)
         VALUES (?, ?, 'epubcfi(/6/4!/4/2,/1:0,/1:9)', 'a highlight')",
    )
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO journal_entries (user_id, book_uuid, body_md) VALUES (?, ?, 'notes')")
        .bind(user_id)
        .bind(book_uuid)
        .execute(pool)
        .await
        .unwrap();
}

/// COUNT every soft-ref user-data row keyed on `book_uuid`.
async fn user_data_count(pool: &SqlitePool, book_uuid: &str) -> i64 {
    let mut total = 0i64;
    for table in [
        "reading_progress",
        "user_ratings",
        "shelf_books",
        "annotations",
        "journal_entries",
    ] {
        let sql = format!("SELECT COUNT(*) FROM {table} WHERE book_uuid = ?");
        total += sqlx::query_scalar::<_, i64>(&sql)
            .bind(book_uuid)
            .fetch_one(pool)
            .await
            .unwrap();
    }
    total
}

/// `(filename, scan_key, path)` for one `book_files` row, by `book_id`.
async fn file_row(pool: &SqlitePool, book_id: i64) -> (String, String, Option<String>) {
    sqlx::query_as(
        "SELECT filename, COALESCE(scan_key, ''), path FROM book_files WHERE book_id = ?",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// AC2: after the reindex that observes the move, every path column names
/// the new library-relative path, `books.uuid` is unchanged, and every
/// soft-ref user-data row is still attached. AC1 rides along: the file is
/// never parsed, so the plan carries no `IndexedBook` at all.
#[tokio::test]
async fn moved_file_repoints_every_path_column_and_keeps_identity_and_user_data() {
    let _covers = CoversTempDir::new("sync_moved_paths");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/Dir/book.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let user_id = seed_user(&pool).await;
    seed_user_data(&pool, user_id, &uuid).await;

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: uuid.clone(),
                filename: "New/Home/renamed.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (scan_key, path, uuid_after, is_missing): (String, String, String, i64) =
        sqlx::query_as("SELECT scan_key, path, uuid, is_missing_files FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        scan_key, "New/Home/renamed.epub",
        "AC2: books.scan_key moved"
    );
    assert_eq!(path, "New/Home", "AC2: books.path moved");
    assert_eq!(uuid_after, uuid, "AC2: books.uuid unchanged");
    assert_eq!(is_missing, 0);

    let (filename, file_scan_key, override_path) = file_row(&pool, book_id).await;
    assert_eq!(filename, "renamed", "AC2: book_files.filename moved");
    assert_eq!(
        file_scan_key, "New/Home/renamed.epub",
        "AC2: book_files.scan_key moved"
    );
    assert_eq!(
        override_path, None,
        "a native row keeps inheriting books.path rather than gaining an override"
    );
    assert_eq!(
        user_data_count(&pool, &uuid).await,
        5,
        "AC2: progress / rating / shelf / annotation / journal all stay attached"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "AC1: a move mints no ledger row"
    );
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

/// A move must not touch `books.last_modified`: nothing a client can see
/// changed, and bumping it would push a whole reorganized library through
/// every device's changed-since feed.
#[tokio::test]
async fn moved_file_does_not_bump_last_modified() {
    let _covers = CoversTempDir::new("sync_moved_last_modified");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    // Backdate so a "now" bump is unmistakable.
    sqlx::query("UPDATE books SET last_modified = 1000 WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid,
                filename: "New/book.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let last_modified: i64 = sqlx::query_scalar("SELECT last_modified FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(last_modified, 1000, "a relocation is not a content change");
}

/// AC10: a relocation the `idx_books_scan_key` unique index would refuse is
/// skipped per-book — the sync still commits, and the skipped book is left
/// exactly as it was rather than half-moved.
#[tokio::test]
async fn moved_file_onto_a_scan_key_another_book_holds_is_skipped_without_failing_the_sync() {
    let _covers = CoversTempDir::new("sync_moved_collision");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mover_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    let squatter_id = seed_book_with_file(&pool, library_id, "New/book.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(mover_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid,
                filename: "New/book.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let (mover_scan_key, mover_path, mover_file_scan_key): (String, String, String) =
        sqlx::query_as(
            "SELECT b.scan_key, b.path, COALESCE(bf.scan_key, '') FROM books b
               JOIN book_files bf ON bf.book_id = b.id WHERE b.id = ?",
        )
        .bind(mover_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        mover_scan_key, "Old/book.epub",
        "AC10: the refused relocation left books.scan_key alone"
    );
    assert_eq!(mover_path, "Old", "AC10: books.path is untouched too");
    // The two seeded books share a `book_files.filename` of "book", so only
    // the file row's own scan_key distinguishes moved from not-moved here.
    assert_eq!(
        mover_file_scan_key, "Old/book.epub",
        "AC10: declining writes nothing at all — not even the file row"
    );
    let squatter_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(squatter_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        squatter_scan_key, "New/book.epub",
        "the holder is untouched"
    );
}

/// A moved uuid that resolves to neither a `books` row nor a ledger entry
/// (a concurrent delete between the diff and the write) is skipped, not
/// fatal — the same TOCTOU tolerance the Changed path already has.
#[tokio::test]
async fn moved_file_with_an_unresolvable_uuid_is_skipped_without_failing_the_sync() {
    let _covers = CoversTempDir::new("sync_moved_unresolvable");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_scan_root(&pool).await;

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "not-a-known-uuid".into(),
                filename: "New/book.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 0);
}
