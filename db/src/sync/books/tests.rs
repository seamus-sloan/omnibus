//! Direct unit tests for the `sync/books` write-path helpers. Exercises
//! each transactional helper in isolation against an in-memory DB — the
//! integration-style `replace_books` tests in `sync/tests.rs` only cover
//! the happy path end-to-end, not the per-helper contracts asserted here
//! (fileless ghosting, per-book link wipe, in-place update, insert).

use omnibus_shared::{Contributor, EbookMetadata, Identifier};
use sqlx::SqlitePool;

use super::shared::{insert_book_row, insert_metadata_links};
use super::{
    collect_entity_alias_maps, sync_books, sync_changed, sync_new, sync_removed,
    wipe_per_book_link_rows, EntityAliasMaps, SyncPlan,
};
use crate::cleanup::{apply_merge_authors, apply_merge_series, apply_merge_tags};
use crate::ebook::IndexedBook;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{
    author_id_by_name, count_rows, indexed, series_id_by_name, CoversTempDir,
};

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
    insert_metadata_links(
        &mut tx,
        inserted.book_id,
        &b.metadata,
        &EntityAliasMaps::default(),
    )
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
    let covers = sync_new(
        &mut tx,
        library_id,
        "/lib",
        &new_books,
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
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
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &changed,
        &EntityAliasMaps::default(),
        |_| {},
    )
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
    let covers = sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    assert!(covers.is_empty());
}

/// Look up a `tags.id` by name. Panics on miss — test helper only.
async fn tag_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `true` when `table.name = ? table` has no row for the given name.
async fn taxonomy_row_missing(pool: &SqlitePool, table: &str, name: &str) -> bool {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE name = ?");
    count_rows_bound(pool, &sql, name).await == 0
}

async fn count_rows_bound(pool: &SqlitePool, sql: &str, bind: &str) -> i64 {
    sqlx::query_scalar(sql)
        .bind(bind)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// `true` when `book_id` is linked to `entity_id` in `link_table` via `col`.
async fn is_linked(
    pool: &SqlitePool,
    link_table: &str,
    col: &str,
    book_id: i64,
    entity_id: i64,
) -> bool {
    let sql = format!("SELECT COUNT(*) FROM {link_table} WHERE book = ? AND {col} = ?");
    let n: i64 = sqlx::query_scalar(&sql)
        .bind(book_id)
        .bind(entity_id)
        .fetch_one(pool)
        .await
        .unwrap();
    n == 1
}

/// #964 AC2: merging an author, series, and tag away, then reindexing the
/// book whose file still names the merged-away entities (`sync_changed`,
/// the real `Task::Scan` write path for an unchanged-on-disk-but-reparsed
/// file), must resolve every one of them to the surviving canonical row —
/// never mint a fresh `authors`/`series`/`tags` row for a name a completed
/// merge already absorbed.
#[tokio::test]
async fn sync_changed_reindex_does_not_resurrect_a_merged_away_author_series_or_tag() {
    let _covers = CoversTempDir::new("resurrection_guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let source = indexed(
        "source.epub",
        Some("Source Book"),
        &["John Smith"],
        &["Sci-Fi"],
        Some(("The Wheel of Time", "1")),
        None,
    );
    let canonical = indexed(
        "canonical.epub",
        Some("Canonical Book"),
        &["J. Smith"],
        &["Science Fiction"],
        Some(("Wheel of Time", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_new(
        &mut tx,
        library_id,
        "/lib",
        &[source, canonical],
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let source_book_id: i64 =
        sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'source.epub'")
            .fetch_one(&pool)
            .await
            .unwrap();

    let author_source_id = author_id_by_name(&pool, "John Smith").await;
    let author_canonical_id = author_id_by_name(&pool, "J. Smith").await;
    let series_source_id = series_id_by_name(&pool, "The Wheel of Time").await;
    let series_canonical_id = series_id_by_name(&pool, "Wheel of Time").await;
    let tag_source_id = tag_id_by_name(&pool, "Sci-Fi").await;
    let tag_canonical_id = tag_id_by_name(&pool, "Science Fiction").await;

    apply_merge_authors(&pool, &[author_source_id], author_canonical_id, None, None)
        .await
        .unwrap();
    apply_merge_series(&pool, &[series_source_id], series_canonical_id, None, None)
        .await
        .unwrap();
    apply_merge_tags(&pool, &[tag_source_id], tag_canonical_id, None, None)
        .await
        .unwrap();

    // A second reindex of the source book: its file on disk never changed,
    // so the parser still reports the names the merges just absorbed.
    let reparsed = [indexed(
        "source.epub",
        Some("Source Book"),
        &["John Smith"],
        &["Sci-Fi"],
        Some(("The Wheel of Time", "1")),
        None,
    )];
    let mut tx = pool.begin().await.unwrap();
    // #1985: the caller now pre-resolves the batch's alias map itself,
    // reflecting the merges just applied above, instead of `sync_changed`
    // resolving it per book internally.
    let alias_maps = collect_entity_alias_maps(&mut tx, &reparsed, &[])
        .await
        .unwrap();
    sync_changed(&mut tx, library_id, "/lib", &reparsed, &alias_maps, |_| {})
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(
        taxonomy_row_missing(&pool, "authors", "John Smith").await,
        "AC2: the merged-away author must not be recreated"
    );
    assert!(
        taxonomy_row_missing(&pool, "series", "The Wheel of Time").await,
        "AC2: the merged-away series must not be recreated"
    );
    assert!(
        taxonomy_row_missing(&pool, "tags", "Sci-Fi").await,
        "AC2: the merged-away tag must not be recreated"
    );

    assert!(
        is_linked(
            &pool,
            "books_authors_link",
            "author",
            source_book_id,
            author_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical author"
    );
    assert!(
        is_linked(
            &pool,
            "books_series_link",
            "series",
            source_book_id,
            series_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical series"
    );
    assert!(
        is_linked(
            &pool,
            "books_tags_link",
            "tag",
            source_book_id,
            tag_canonical_id
        )
        .await,
        "AC1: reindexed book resolves to the canonical tag"
    );
}

/// AC3: with no alias recorded, `sync_changed` behaves exactly as before —
/// the reindexed name resolves to its own (unmerged) row, not some other
/// entity's.
#[tokio::test]
async fn sync_changed_reindex_leaves_unmerged_entities_unaffected() {
    let _covers = CoversTempDir::new("resurrection_guard_unaffected");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let book = indexed(
        "plain.epub",
        Some("Plain Book"),
        &["Ordinary Author"],
        &["Ordinary Tag"],
        Some(("Ordinary Series", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_new(
        &mut tx,
        library_id,
        "/lib",
        &[book],
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'plain.epub'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let author_id = author_id_by_name(&pool, "Ordinary Author").await;
    let series_id = series_id_by_name(&pool, "Ordinary Series").await;
    let tag_id = tag_id_by_name(&pool, "Ordinary Tag").await;

    let reparsed = indexed(
        "plain.epub",
        Some("Plain Book"),
        &["Ordinary Author"],
        &["Ordinary Tag"],
        Some(("Ordinary Series", "1")),
        None,
    );
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[reparsed],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM authors").await,
        1,
        "no extra author row minted"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM series").await,
        1,
        "no extra series row minted"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM tags").await,
        1,
        "no extra tag row minted"
    );
    assert!(is_linked(&pool, "books_authors_link", "author", book_id, author_id).await);
    assert!(is_linked(&pool, "books_series_link", "series", book_id, series_id).await);
    assert!(is_linked(&pool, "books_tags_link", "tag", book_id, tag_id).await);
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
    // The anchor row's scan_key must be set on insert, not left for a boot backfill.
    let file_scan_key: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM book_files WHERE book_id = ?")
            .bind(inserted.book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        file_scan_key.as_deref(),
        Some("solo.epub"),
        "book_files.scan_key is set on insert, matching books.scan_key"
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
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[changed],
        &EntityAliasMaps::default(),
        |_| {},
    )
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

/// `page_count` round-trips the same way `word_count` does (#1593):
/// `insert_book_row` persists the CBZ parser's page count, and
/// `sync_changed` refreshes it on re-parse (a book edited to add pages).
#[tokio::test]
async fn page_count_persists_on_insert_and_refreshes_on_change() {
    let _covers = CoversTempDir::new("sync_page_count_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let mut b = indexed("pc.cbz", Some("Paged"), &[], &[], None, None);
    b.metadata.page_count = Some(12);
    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    let stored: Option<i64> = sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, Some(12), "insert persists the page count");

    // Same filename (→ same scan_key), new page count — the Changed path.
    let mut changed = indexed("pc.cbz", Some("Paged"), &[], &[], None, None);
    changed.metadata.page_count = Some(20);
    let mut tx = pool.begin().await.unwrap();
    sync_changed(
        &mut tx,
        library_id,
        "/lib",
        &[changed],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    let refreshed: Option<i64> = sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(inserted.book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(refreshed, Some(20), "the update refreshes page_count");
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

// ── #1536: the Moved bucket (stat-pair relocation writer) ────────────

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

/// AC7: moving a cross-format attachment updates its own `merged_uuids`
/// and `book_files` rows — including the location override, which is what
/// the read path resolves the file through — and leaves the target book's
/// own `scan_key` / `path` (which name a *different* file) untouched.
#[tokio::test]
async fn moved_attachment_updates_the_ledger_and_leaves_the_target_books_row_alone() {
    let _covers = CoversTempDir::new("sync_moved_attachment");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    // Plant an M4B attachment on the same book, keyed on its own path.
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, library_path, path, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', '/lib', 'Audio/Old', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "ledger-uuid".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let ledger_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM merged_uuids WHERE uuid = 'ledger-uuid'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        ledger_scan_key, "Audio/New/book.m4b",
        "AC7: the ledger follows its file"
    );
    let (file_scan_key, override_path): (String, String) = sqlx::query_as(
        "SELECT scan_key, path FROM book_files WHERE book_id = ? AND format = 'M4B'",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(file_scan_key, "Audio/New/book.m4b");
    assert_eq!(
        override_path, "Audio/New",
        "AC7: an attachment's location override follows it too"
    );
    let (target_scan_key, target_path): (String, String) =
        sqlx::query_as("SELECT scan_key, path FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        target_scan_key, "Ebooks/book.epub",
        "AC7: the target book's own scan_key names a different file and must not move"
    );
    assert_eq!(target_path, "Ebooks");
}

/// AC8: for a book holding two EPUBs (via `merge_books`), moving either
/// file is detected and written independently — the anchor moves through
/// `books`, the merged-in file through its ledger row, and neither
/// disturbs the other.
#[tokio::test]
async fn moved_file_of_a_two_epub_book_relocates_only_its_own_row() {
    let _covers = CoversTempDir::new("sync_moved_two_epub");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (anchor_uuid, merged_uuid, book_id) = seed_merged_two_epub_book(&pool).await;

    // Move the merged-in file only.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: merged_uuid.clone(),
                filename: "Moved/two.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        anchor_scan_key, "A/one.epub",
        "AC8: the sibling anchor row is left untouched"
    );
    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["A/one.epub".to_string(), "Moved/two.epub".to_string()],
        "AC8: only the moved file's own book_files row relocated"
    );

    // Now move the anchor file too, in a second scan.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: anchor_uuid,
                filename: "Moved/one.epub".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(anchor_scan_key, "Moved/one.epub", "AC8: the anchor moved");
    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["Moved/one.epub".to_string(), "Moved/two.epub".to_string()],
    );
}

/// AC8, the both-at-once half: relocating each file of a two-EPUB book in
/// one scan lands both.
#[tokio::test]
async fn moving_both_files_of_a_two_epub_book_in_one_scan_relocates_both() {
    let _covers = CoversTempDir::new("sync_moved_two_epub_both");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (anchor_uuid, merged_uuid, book_id) = seed_merged_two_epub_book(&pool).await;

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![
                crate::sync::MovedFile {
                    uuid: anchor_uuid,
                    filename: "Moved/one.epub".into(),
                },
                crate::sync::MovedFile {
                    uuid: merged_uuid,
                    filename: "Moved/two.epub".into(),
                },
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(
        scan_keys_for(&pool, book_id).await,
        vec!["Moved/one.epub".to_string(), "Moved/two.epub".to_string()],
    );
    let anchor_scan_key: String = sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(anchor_scan_key, "Moved/one.epub");
}

/// Seed the two-EPUB shape through the real `merge_books` path and return
/// `(anchor uuid, merged-in ledger uuid, surviving books.id)`.
async fn seed_merged_two_epub_book(pool: &SqlitePool) -> (String, String, i64) {
    let library_id = seed_scan_root(pool).await;
    let target_id = seed_book_with_file(pool, library_id, "A/one.epub").await;
    let source_id = seed_book_with_file(pool, library_id, "B/two.epub").await;
    let anchor_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(pool)
        .await
        .unwrap();
    let source_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(source_id)
        .fetch_one(pool)
        .await
        .unwrap();
    crate::merge::merge_books(pool, &source_uuid, &anchor_uuid, None)
        .await
        .unwrap();
    (anchor_uuid, source_uuid, target_id)
}

/// Every `book_files.scan_key` under one book, sorted.
async fn scan_keys_for(pool: &SqlitePool, book_id: i64) -> Vec<String> {
    let mut keys: Vec<String> =
        sqlx::query_scalar("SELECT COALESCE(scan_key, '') FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .fetch_all(pool)
            .await
            .unwrap();
    keys.sort();
    keys
}

/// End-to-end through the real classifier: three consecutive reindexes
/// after a move converge, and the move never reaches Removed — so the
/// mass-missing breaker never sees it (AC3) and no ledger row is minted
/// (AC1).
#[tokio::test]
async fn three_reindexes_after_a_stat_matched_move_converge_without_ghosting() {
    let _covers = CoversTempDir::new("sync_moved_convergence");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    // Give the seeded file a real stat pair so it can be matched at all.
    sqlx::query("UPDATE book_files SET mtime_epoch = 4242, size_bytes = 9999 WHERE book_id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let uuid_before: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    let disk = [crate::ebook::StatEntry {
        filename: "New/book.epub".into(),
        scan_key: "New/book.epub".into(),
        mtime_epoch: 4242,
        size_bytes: 9999,
        error: None,
    }];

    for pass in 0..3 {
        let db_rows = crate::books::list_indexed_rows_for_formats(&pool, "/lib", &["EPUB"])
            .await
            .unwrap();
        let diff = diff_library(&disk, &db_rows, std::path::Path::new("/lib"), true);
        if pass == 0 {
            assert_eq!(diff.moved.len(), 1, "pass 0: the move is detected");
            assert!(diff.removed.is_empty(), "AC3: nothing ghosts");
            assert!(diff.new.is_empty(), "AC1: no Phase-B parse target");
        } else {
            assert!(diff.moved.is_empty(), "pass {pass}: already settled");
            assert_eq!(diff.unchanged.len(), 1, "pass {pass}: classifies Unchanged");
        }

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
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
        assert_eq!(scan_key, "New/book.epub", "pass {pass}: scan_key settled");
        assert_eq!(is_missing, 0, "pass {pass}: never flagged missing");
        assert_eq!(
            count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
            0,
            "pass {pass}: no ledger row minted"
        );
    }
}

/// An attachment's `book_files.path` override is the *only* column that
/// points at its file — the target book's `books.path` names a different
/// one — so the relocation must write it even on a row that somehow
/// carries none, rather than leaving the file inheriting a directory it
/// does not live in.
#[tokio::test]
async fn moved_attachment_without_a_location_override_gains_one_pointing_at_its_new_home() {
    let _covers = CoversTempDir::new("sync_moved_attach_null_path");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ledger-uuid', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "ledger-uuid".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let override_path: Option<String> =
        sqlx::query_scalar("SELECT path FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        override_path.as_deref(),
        Some("Audio/New"),
        "the attachment must name its own directory, never inherit the target's"
    );
}

/// Replaying the same Moved plan is a no-op: the second pass reads the
/// already-moved `scan_key` as its "old" value and rewrites the same
/// values. Worth pinning — a sync plan is data, and a retried write path
/// must not corrupt a book that already landed.
#[tokio::test]
async fn replaying_the_same_moved_plan_is_idempotent() {
    let _covers = CoversTempDir::new("sync_moved_idempotent");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Old/book.epub").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();

    for pass in 0..2 {
        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                moved: vec![crate::sync::MovedFile {
                    uuid: uuid.clone(),
                    filename: "New/book.epub".into(),
                }],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let (scan_key, path): (String, String) =
            sqlx::query_as("SELECT scan_key, path FROM books WHERE id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(scan_key, "New/book.epub", "pass {pass}");
        assert_eq!(path, "New", "pass {pass}");
        assert_eq!(book_files_count(&pool, book_id).await, 1, "pass {pass}");
        assert_eq!(
            scan_keys_for(&pool, book_id).await,
            vec!["New/book.epub".to_string()],
            "pass {pass}"
        );
    }
}

/// A moved attachment must not land on a `(library_path, scan_key)` another
/// ledger row already holds. `merged_uuids` has only a non-unique index
/// there, so the UPDATE cannot abort the transaction — it would silently
/// leave two rows sharing one key, and `find_attachment_by_scan_key` uses
/// `fetch_optional`, so a later scan would resolve the attachment
/// arbitrarily. Reachable because a ledger row whose `book_files` row was
/// dropped (the Changed path wipes a book's rows per format) is excluded
/// from the diff's merged projection, so its path is offered as an arrival.
/// Mirrors the `books`-side `idx_books_scan_key` pre-check.
#[tokio::test]
async fn moved_attachment_onto_a_scan_key_another_ledger_row_holds_is_skipped() {
    let _covers = CoversTempDir::new("sync_moved_ledger_collision");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let book_id = seed_book_with_file(&pool, library_id, "Ebooks/book.epub").await;
    // The attachment that is about to move, with its backing file row.
    sqlx::query(
        "INSERT INTO book_files
            (book_id, format, filename, size_bytes, mtime_epoch, scan_key, library_path, path, ordinal)
         VALUES (?, 'M4B', 'book', 4242, 777, 'Audio/Old/book.m4b', '/lib', 'Audio/Old', 1)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('mover', ?, 'M4B', '/lib', 'Audio/Old/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    // A stale ledger row already holding the destination — no `book_files`
    // row behind it, which is exactly why the diff can offer that path.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('squatter', ?, 'M4B', '/lib', 'Audio/New/book.m4b')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            moved: vec![crate::sync::MovedFile {
                uuid: "mover".into(),
                filename: "Audio/New/book.m4b".into(),
            }],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let sharing: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM merged_uuids WHERE library_path = '/lib'
           AND scan_key = 'Audio/New/book.m4b'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        sharing, 1,
        "two ledger rows must never share one (library_path, scan_key)"
    );
    let mover_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM merged_uuids WHERE uuid = 'mover'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        mover_scan_key, "Audio/Old/book.m4b",
        "the refused relocation left the ledger row alone"
    );
    let file_scan_key: String =
        sqlx::query_scalar("SELECT scan_key FROM book_files WHERE book_id = ? AND format = 'M4B'")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        file_scan_key, "Audio/Old/book.m4b",
        "declining writes nothing at all — not even the file row"
    );
}

/// Build an `IndexedBook` carrying only a series name + optional explicit
/// index — the shape a scanned "Name #N" library produces.
fn book_with_series(
    filename: &str,
    title: &str,
    series: &str,
    explicit_index: Option<&str>,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: Some(title.into()),
            creators: vec![Contributor {
                name: "Author".into(),
                ..Default::default()
            }],
            series: Some(series.into()),
            series_index: explicit_index.map(Into::into),
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    }
}

/// AC1: a library whose series metadata carries the index inside the name
/// ("Name #1"/"Name #2"/"Name #3") indexes as one series row with three
/// distinctly-indexed books, not three one-book series (#1912).
#[tokio::test]
async fn sync_new_collapses_hash_indexed_series_names_into_one_series_with_per_book_indexes() {
    let _covers = CoversTempDir::new("sync_books_series_hash_index");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                book_with_series("a.epub", "Book 1", "Crowns of Nyaxia #1", None),
                book_with_series("b.epub", "Book 2", "Crowns of Nyaxia #2", None),
                book_with_series("c.epub", "Book 3", "Crowns of Nyaxia #3", None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let series_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series_rows.len(),
        1,
        "the three fragmented names collapse onto one series row: {series_rows:?}"
    );
    assert_eq!(series_rows[0].1, "Crowns of Nyaxia");

    let mut indexes: Vec<f64> = sqlx::query_scalar("SELECT series_index FROM books")
        .fetch_all(&pool)
        .await
        .unwrap();
    indexes.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(indexes, vec![1.0, 2.0, 3.0]);
}

/// AC2: an explicit `series_index` always wins over a number embedded in the
/// series name; the embedded index only fills the gap on a book that has no
/// explicit value of its own (#1912).
#[tokio::test]
async fn sync_new_keeps_explicit_series_index_and_only_fills_the_gap_from_the_embedded_name() {
    let _covers = CoversTempDir::new("sync_books_series_explicit_index");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                book_with_series(
                    "explicit.epub",
                    "Explicit",
                    "Gods and Monsters #2",
                    Some("9"),
                ),
                book_with_series("embedded.epub", "Embedded", "Gods and Monsters #3", None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let series_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(series_count, 1, "both books share one cleaned series row");

    let explicit_index: f64 =
        sqlx::query_scalar("SELECT series_index FROM books WHERE title = 'Explicit'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        explicit_index, 9.0,
        "explicit index wins over the embedded #2"
    );

    let embedded_index: f64 =
        sqlx::query_scalar("SELECT series_index FROM books WHERE title = 'Embedded'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        embedded_index, 3.0,
        "the embedded index fills the gap when no explicit value was scanned"
    );
}

// ── #1985: alias-lookup query count is batch-bounded, not per-book ────────

/// Counts `tracing` events sqlx emits (target `"sqlx::query"`, one per
/// executed statement) whose `db.statement` field mentions `entity_aliases` —
/// i.e. a `resolve_entity_aliases` lookup specifically, not every query the
/// sync writes. Mirrors the `QueryCounter` pattern in
/// `db/src/epub_rewrite/tests.rs`, narrowed to the one table this test cares
/// about so the assertion isn't swamped by the per-book `books`/link-table
/// writes a reindex batch also issues.
///
/// Installed as the **global** default (not `tracing::subscriber::set_default`,
/// which is thread-local): `sqlx-sqlite` runs every statement — including
/// each one this counts — on a dedicated per-connection worker thread (see
/// `sqlx_sqlite::connection::worker`), so a thread-local override on the
/// test's own async task never sees them. A global default is process-wide
/// and permanent for the rest of the test binary, which is why the
/// assertion below uses a loose upper bound rather than an exact count — a
/// concurrently-running unrelated test's own `entity_aliases` query could
/// add a stray hit to this counter too.
struct EntityAliasQueryCounter(std::sync::Arc<std::sync::atomic::AtomicUsize>);

impl tracing::Subscriber for EntityAliasQueryCounter {
    fn enabled(&self, metadata: &tracing::Metadata<'_>) -> bool {
        metadata.target() == "sqlx::query"
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::span::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::span::Id, _follows: &tracing::span::Id) {}
    fn event(&self, event: &tracing::Event<'_>) {
        struct MentionsEntityAliases(bool);
        impl tracing::field::Visit for MentionsEntityAliases {
            fn record_debug(
                &mut self,
                _field: &tracing::field::Field,
                _value: &dyn std::fmt::Debug,
            ) {
            }
            fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                if field.name().ends_with("statement") && value.contains("entity_aliases") {
                    self.0 = true;
                }
            }
        }
        let mut visitor = MentionsEntityAliases(false);
        event.record(&mut visitor);
        if visitor.0 {
            self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    fn enter(&self, _span: &tracing::span::Id) {}
    fn exit(&self, _span: &tracing::span::Id) {}
}

/// #1985 AC3: a multi-book reindex batch issues an `entity_aliases` lookup
/// count bounded by the touched `CleanupKind`s (author, series, tag — a
/// small constant), not by book count. The old per-book shape issued
/// `3 * BOOK_COUNT` such queries (`insert_author_links`/`insert_tag_links`/
/// `resolve_or_insert_series` each called `resolve_entity_aliases` once per
/// book); the batched shape issues 3 total regardless of `BOOK_COUNT`. The
/// assertion checks `< BOOK_COUNT` rather than `== 3` — see
/// `EntityAliasQueryCounter`'s doc for why an exact count isn't safe to
/// assert against a process-wide subscriber — but `BOOK_COUNT` is picked
/// large enough that the old shape would fail this bound by a wide margin
/// while the new shape passes with room to spare.
#[tokio::test]
async fn sync_books_issues_one_entity_alias_query_per_kind_not_per_book() {
    let _covers = CoversTempDir::new("sync_books_alias_query_count");
    let pool = init_db("sqlite::memory:").await.unwrap();

    const BOOK_COUNT: usize = 20;
    let new_books: Vec<IndexedBook> = (0..BOOK_COUNT)
        .map(|i| {
            let filename = format!("book-{i}.epub");
            let title = format!("Book {i}");
            let author = format!("Author {i}");
            let tag = format!("Tag {i}");
            let series = format!("Series {i}");
            indexed(
                &filename,
                Some(&title),
                &[&author],
                &[&tag],
                Some((&series, "1")),
                None,
            )
        })
        .collect();

    let count = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    // Best-effort: a prior test in this binary may have already claimed the
    // global default (it can only be set once per process). If so, this
    // counter simply stays at 0 and the loose bound below still holds —
    // it just stops being a meaningful check for this particular run.
    let _ = tracing::subscriber::set_global_default(EntityAliasQueryCounter(count.clone()));
    // Interest in the `sqlx::query` callsite is cached process-wide the
    // first time it fires and is *not* refreshed by installing a new
    // default alone — `init_db`'s migrations above already fired it under
    // whatever (no-op) dispatcher preceded this one, which would otherwise
    // leave it permanently cached "never interested".
    tracing::callsite::rebuild_interest_cache();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books,
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let queries = count.load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        queries < BOOK_COUNT,
        "expected an entity_aliases query count bounded by the touched \
         CleanupKinds (author, series, tag — 3, not {BOOK_COUNT}), got \
         {queries} for a {BOOK_COUNT}-book batch"
    );
}
