//! Direct unit tests for the `sync/books` write-path helpers. Exercises
//! each transactional helper in isolation against an in-memory DB — the
//! integration-style `replace_books` tests in `sync/tests.rs` only cover
//! the happy path end-to-end, not the per-helper contracts asserted here
//! (fileless ghosting, per-book link wipe, in-place update, insert).

use omnibus_shared::{Contributor, EbookMetadata, Identifier};
use sqlx::SqlitePool;

use super::shared::{insert_book_row, insert_metadata_links};
use super::{sync_books, sync_changed, sync_new, sync_removed, wipe_per_book_link_rows, SyncPlan};
use crate::ebook::IndexedBook;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed, CoversTempDir};

/// Insert a `scan_roots` row for `/lib` and return its id — the
/// `library_id` every bucket helper needs.
async fn seed_scan_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Build a fully-populated `IndexedBook` so a single seed touches every
/// per-book link table: authors, tags, series, publisher, language, and
/// identifiers. Used by the link-wipe test to prove all 7 tables clear.
fn book_with_all_links(filename: &str, title: &str) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            publisher: Some("Tor".into()),
            language: Some("en".into()),
            creators: vec![Contributor {
                name: "Ada Lovelace".into(),
                ..Default::default()
            }],
            subjects: vec!["fiction".into()],
            series: Some("Saga".into()),
            series_index: Some("1".into()),
            identifiers: vec![Identifier {
                value: "9780000000000".into(),
                scheme: Some("ISBN".into()),
            }],
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// Seed one native book (canonical `books` row + its `book_files` row +
/// all per-book link rows) through the real write helpers, returning its
/// `books.id`. Local to the test module so production code stays lean.
async fn seed_book_with_file(pool: &SqlitePool, library_id: i64, filename: &str) -> i64 {
    let b = book_with_all_links(filename, "Seeded");
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    insert_metadata_links(&mut tx, inserted.book_id, &b.metadata)
        .await
        .unwrap();
    tx.commit().await.unwrap();
    inserted.book_id
}

/// COUNT `book_files` rows for one `book_id`.
async fn book_files_count(pool: &SqlitePool, book_id: i64) -> i64 {
    sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `sync_removed` ghosts a book (F2): the file is gone, so its
/// `book_files` row is dropped, but the durable `books` row survives so
/// the uuid — and any user data keyed on it — persists.
#[tokio::test]
async fn sync_removed_retains_books_row_and_removes_book_files_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "file present pre-ghost"
    );

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[uuid]).await.unwrap();
    tx.commit().await.unwrap();

    let books_still: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        books_still, 1,
        "the books row is retained (durable identity)"
    );
    assert_eq!(
        book_files_count(&pool, book_id).await,
        0,
        "the book_files row is removed"
    );
    // The retained row is flagged missing so F10 GC can later reap it.
    let flagged: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flagged, 1, "retained row is flagged missing");
}

/// A book whose own ebook file is removed, but which still holds a
/// cross-format attachment (a different format's file, recorded in
/// `merged_uuids` and still present), is not flagged missing — the surviving
/// file means the book isn't actually fileless.
#[tokio::test]
async fn sync_removed_does_not_flag_missing_when_a_cross_format_attachment_survives() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Forge a surviving cross-format attachment: a book_files row in a
    // different format, backed by its own merged_uuids ledger entry.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'M4B', 'a', 1000, 100, 'a.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('attached-m4b', ?, 'M4B', '/audio', 'a.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[uuid]).await.unwrap();
    tx.commit().await.unwrap();

    // The EPUB row is dropped, the M4B survives, and the book must not be
    // flagged missing since it still holds a file.
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        0,
        "the book's own native file row is dropped"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1,
        "the cross-format attachment survives"
    );
    let is_missing: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        is_missing, 0,
        "a surviving cross-format attachment means the book isn't fileless"
    );
}

/// One `sync_removed` call ghosts every uuid in the batch — proving the
/// batched DELETE + UPDATE covers N books in a single invocation.
#[tokio::test]
async fn sync_removed_ghosts_multiple_books_in_one_call() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut uuids = Vec::new();
    for i in 0..3 {
        let book_id = seed_book_with_file(&pool, library_id, &format!("b{i}.epub")).await;
        let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        uuids.push(uuid);
    }
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        3
    );

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &uuids).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        3,
        "all three books rows retained"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0,
        "all three book_files rows dropped in one call"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books WHERE is_missing_files = 1"
        )
        .await,
        3,
        "all three rows flagged missing"
    );
}

/// `sync_removed` is a no-op for an empty batch (early return) and does
/// not touch any rows.
#[tokio::test]
async fn sync_removed_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "keep.epub").await;

    let mut tx = pool.begin().await.unwrap();
    sync_removed(&mut tx, library_id, &[]).await.unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "empty removed batch leaves the file row intact"
    );
}

/// `wipe_per_book_link_rows` clears all seven per-book tables for the
/// given `book_id` (the format-scoped `book_files` row plus the six link
/// tables) so `sync_changed` can re-insert without hitting UNIQUE
/// constraints.
#[tokio::test]
async fn wipe_per_book_link_rows_clears_all_seven_tables_for_the_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;

    // Pre-condition: every per-book table has a row for this book.
    for (table, col) in [
        ("book_files", "book_id"),
        ("book_identifiers", "book_id"),
        ("books_authors_link", "book"),
        ("books_tags_link", "book"),
        ("books_publishers_link", "book"),
        ("books_series_link", "book"),
        ("books_languages_link", "book"),
    ] {
        let n = count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE {col} = {book_id}"),
        )
        .await;
        assert_eq!(n, 1, "{table} should have a seeded row before the wipe");
    }

    let mut tx = pool.begin().await.unwrap();
    // The seed file is an EPUB, so wipe that format's book_files row.
    wipe_per_book_link_rows(&mut tx, book_id, "EPUB")
        .await
        .unwrap();
    tx.commit().await.unwrap();

    for (table, col) in [
        ("book_files", "book_id"),
        ("book_identifiers", "book_id"),
        ("books_authors_link", "book"),
        ("books_tags_link", "book"),
        ("books_publishers_link", "book"),
        ("books_series_link", "book"),
        ("books_languages_link", "book"),
    ] {
        let n = count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM {table} WHERE {col} = {book_id}"),
        )
        .await;
        assert_eq!(n, 0, "{table} should be empty for the book after the wipe");
    }
}

/// `sync_new` inserts a brand-new book: a canonical `books` row, its
/// `book_files` row, and every per-book link row.
#[tokio::test]
async fn sync_new_inserts_a_new_book_and_its_link_rows() {
    let _covers = CoversTempDir::new("sync_new_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let new_books = vec![book_with_all_links("fresh.epub", "Fresh")];
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_new(&mut tx, library_id, "/lib", &new_books, &[], || {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(covers.is_empty(), "no cover was supplied");
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'fresh.epub'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Fresh");
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "book_files inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1,
        "author link inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_series_link WHERE book = {book_id}")
        )
        .await,
        1,
        "series link inserted"
    );
    // FTS row was written from the inserted rows.
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_fts WHERE rowid = {book_id}")
        )
        .await,
        1,
        "FTS row inserted"
    );
}

/// `sync_changed` updates an existing book in place — the `books.id` and
/// `books.uuid` are preserved while the scalar columns and link rows are
/// rewritten from the fresh parse.
#[tokio::test]
async fn sync_changed_updates_an_existing_book_row_in_place() {
    let _covers = CoversTempDir::new("sync_changed_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "a.epub").await;
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Same filename (→ same scan_key), new title + author.
    let changed = vec![indexed(
        "a.epub",
        Some("Updated Title"),
        &["New Author"],
        &[],
        None,
        None,
    )];
    let mut tx = pool.begin().await.unwrap();
    sync_changed(&mut tx, library_id, "/lib", &changed, || {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    // Exactly one row, same id + uuid, refreshed scalars.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "changed updates in place, not insert"
    );
    let (id_after, uuid_after, title_after): (i64, String, String) =
        sqlx::query_as("SELECT id, uuid, title FROM books WHERE scan_key = 'a.epub'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(id_after, book_id, "books.id preserved across change");
    assert_eq!(
        uuid_after, uuid_before,
        "books.uuid preserved across change"
    );
    assert_eq!(title_after, "Updated Title", "scalar columns refreshed");

    // The author link was wiped-and-rewritten to the new author only.
    let authors: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM authors a
         JOIN books_authors_link l ON l.author = a.id
         WHERE l.book = ?",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(authors, vec!["New Author".to_string()], "links rewritten");
}

/// `sync_changed` with an empty batch is a no-op and returns no cover
/// triples (early return before the id pre-fetch).
#[tokio::test]
async fn sync_changed_is_a_noop_for_empty_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_changed(&mut tx, library_id, "/lib", &[], || {})
        .await
        .unwrap();
    tx.commit().await.unwrap();
    assert!(covers.is_empty());
}

/// A helper on the `IndexedBook` boundary: `insert_book_row` returns the
/// new id + a freshly minted uuid, and writes exactly one `book_files`
/// row alongside the `books` row.
#[tokio::test]
async fn insert_book_row_writes_books_and_book_files_and_mints_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let b = indexed("solo.epub", Some("Solo"), &[], &[], None, None);

    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(!inserted.uuid.is_empty(), "a uuid is minted");
    let (uuid, scan_key): (String, String) =
        sqlx::query_as("SELECT uuid, scan_key FROM books WHERE id = ?")
            .bind(inserted.book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(uuid, inserted.uuid, "returned uuid matches the stored row");
    assert_eq!(scan_key, "solo.epub", "scan_key is the relative path");
    assert_eq!(
        book_files_count(&pool, inserted.book_id).await,
        1,
        "exactly one book_files row"
    );
}

/// `word_count` round-trips through both the insert and the in-place update:
/// `insert_book_row` persists the estimate from the `IndexedBook`, and
/// `sync_changed` refreshes it on re-parse (a book edited to a longer text).
#[tokio::test]
async fn word_count_persists_on_insert_and_refreshes_on_change() {
    let _covers = CoversTempDir::new("sync_word_count_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let mut b = indexed("wc.epub", Some("Counted"), &[], &[], None, None);
    b.word_count = Some(1000);
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let stored: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Some(1000), "insert persists the estimate");

    // Same filename (→ same scan_key), new word count — the Changed path.
    let mut changed = indexed("wc.epub", Some("Counted"), &[], &[], None, None);
    changed.word_count = Some(2500);
    let mut tx = pool.begin().await.unwrap();
    sync_changed(&mut tx, library_id, "/lib", &[changed], || {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let refreshed: Option<i64> = sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refreshed, Some(2500), "the update refreshes word_count");
}

// ── Moved/renamed ebook relocation ───────────────────────────────────

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

#[tokio::test]
async fn insert_chapters_propagates_db_error_when_table_missing() {
    // `SyncError` (crate-internal, shared by the ebook and audiobook sync
    // writers) has no direct pool-level entry point of its own — it's
    // produced deep inside the audiobook chapter writer. Dropping the
    // target table mid-transaction forces the same `sqlx::Error` passthrough
    // a closed pool would, without needing a second in-memory DB handle.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE file_chapters")
        .execute(&mut *tx)
        .await
        .unwrap();
    let part = crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "01.mp3".into(),
        size_bytes: 100,
        mtime_epoch: 0,
        duration_seconds: 10.0,
    };
    let err =
        crate::sync::audiobooks::insert_chapters(&mut tx, 1, &[], std::slice::from_ref(&part))
            .await
            .unwrap_err();
    assert!(matches!(err, super::SyncError::Db(_)));
}
