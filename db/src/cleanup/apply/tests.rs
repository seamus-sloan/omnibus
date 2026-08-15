//! Round-trip coverage for the apply/undo primitives: one happy-path
//! apply-then-undo test per primitive, plus the error variants each can
//! return. All tests run against `sqlite::memory:` per
//! [`crate::pool::init_db`].

use sqlx::SqlitePool;

use omnibus_shared::MetadataOverrides;

use super::super::undo::undo;
use super::*;
use crate::pool::init_db;

// ---------------------------------------------------------------------------
// Seed helpers
// ---------------------------------------------------------------------------

async fn new_pool() -> SqlitePool {
    init_db("sqlite::memory:").await.unwrap()
}

async fn seed_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_user(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('admin', 'x', 1) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(format!("/lib/{uuid}"))
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn insert_author(pool: &SqlitePool, name: &str, sort: Option<&str>) -> i64 {
    sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(sort)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_series(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO series (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn insert_tag(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar("INSERT INTO tags (name) VALUES (?) RETURNING id")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn link_author(pool: &SqlitePool, book_id: i64, author_id: i64, position: i64) {
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, ?)")
        .bind(book_id)
        .bind(author_id)
        .bind(position)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_series(pool: &SqlitePool, book_id: i64, series_id: i64) {
    sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?, ?)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_tag(pool: &SqlitePool, book_id: i64, tag_id: i64) {
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(book_id)
        .bind(tag_id)
        .execute(pool)
        .await
        .unwrap();
}

async fn insert_author_photo(
    pool: &SqlitePool,
    author_id: i64,
    source: &str,
    bytes: Option<&[u8]>,
) {
    sqlx::query("INSERT INTO author_photos (author_id, source, mime, bytes) VALUES (?, ?, ?, ?)")
        .bind(author_id)
        .bind(source)
        .bind(bytes.map(|_| "image/png"))
        .bind(bytes)
        .execute(pool)
        .await
        .unwrap();
}

async fn author_photo_source(pool: &SqlitePool, author_id: i64) -> Option<String> {
    sqlx::query_scalar("SELECT source FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn author_row(pool: &SqlitePool, name: &str) -> Option<(i64, Option<String>)> {
    sqlx::query_as("SELECT id, sort FROM authors WHERE name = ?")
        .bind(name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn author_position(pool: &SqlitePool, book_id: i64, author_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT position FROM books_authors_link WHERE book = ? AND author = ?")
        .bind(book_id)
        .bind(author_id)
        .fetch_optional(pool)
        .await
        .unwrap()
}

async fn count_rows(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}

async fn fts_authors(pool: &SqlitePool, book_id: i64) -> String {
    sqlx::query_scalar("SELECT authors FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn fts_tags(pool: &SqlitePool, book_id: i64) -> String {
    sqlx::query_scalar("SELECT tags FROM books_fts WHERE rowid = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

async fn is_ignored_author(pool: &SqlitePool, name: &str) -> bool {
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    count > 0
}

async fn alias_canonical(pool: &SqlitePool, kind: &str, alias_name: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT canonical_id FROM entity_aliases WHERE kind = ? AND alias_name = ?")
        .bind(kind)
        .bind(alias_name)
        .fetch_optional(pool)
        .await
        .unwrap()
}

// ---------------------------------------------------------------------------
// apply_merge_authors
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_merge_authors_moves_link_writes_alias_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Sandy Brandon", None).await;
    let canonical = insert_author(&pool, "Brandon Sandy", Some("Sandy, Brandon")).await;
    link_author(&pool, book, source, 2).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 1);
    assert_eq!(
        author_position(&pool, book, canonical).await,
        Some(2),
        "canonical had no prior link, so it inherits the source's position"
    );
    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        Some(canonical)
    );
    assert!(fts_authors(&pool, book).await.contains("Brandon Sandy"));
    assert!(!fts_authors(&pool, book).await.contains("Sandy Brandon"));

    undo(&pool, log_id).await.unwrap();

    let (restored_id, restored_sort) = author_row(&pool, "Sandy Brandon").await.unwrap();
    assert_eq!(restored_sort, None);
    assert_eq!(author_position(&pool, book, restored_id).await, Some(2));
    assert_eq!(
        author_position(&pool, book, canonical).await,
        None,
        "the canonical's merge-created link must be removed on undo"
    );
    assert_eq!(
        alias_canonical(&pool, "author", "Sandy Brandon").await,
        None
    );
    assert!(fts_authors(&pool, book).await.contains("Sandy Brandon"));
    assert!(!fts_authors(&pool, book).await.contains("Brandon Sandy"));
}

#[tokio::test]
async fn apply_merge_authors_backfills_null_sort_and_undo_clears_it() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Bar Foo", Some("Foo, Bar")).await;
    let canonical = insert_author(&pool, "Foo Bar", None).await;
    link_author(&pool, book, source, 0).await;
    link_author(&pool, book, canonical, 1).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    let (_, canonical_sort) = author_row(&pool, "Foo Bar").await.unwrap();
    assert_eq!(canonical_sort.as_deref(), Some("Foo, Bar"));
    // The book already had a canonical link before the merge, so the
    // source's own link must NOT survive as a second row for the same book.
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book}")
        )
        .await,
        1
    );

    undo(&pool, log_id).await.unwrap();

    let (_, canonical_sort_after_undo) = author_row(&pool, "Foo Bar").await.unwrap();
    assert_eq!(
        canonical_sort_after_undo, None,
        "undo must restore the backfilled sort to NULL, not just to its own prior value"
    );
    assert_eq!(
        author_position(&pool, book, canonical).await,
        Some(1),
        "the canonical's pre-existing link must survive the round trip untouched"
    );
}

#[tokio::test]
async fn apply_merge_authors_reconciles_photos_by_priority_and_undo_restores_both() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_author(&pool, "Manual Source", None).await;
    let canonical = insert_author(&pool, "Letter Canonical", None).await;
    link_author(&pool, book, source, 0).await;
    insert_author_photo(&pool, source, "manual", Some(b"manual-bytes")).await;
    insert_author_photo(&pool, canonical, "letter", None).await;

    let log_id = apply_merge_authors(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(
        author_photo_source(&pool, canonical).await.as_deref(),
        Some("manual"),
        "manual outranks letter, so the canonical adopts the source's photo"
    );

    undo(&pool, log_id).await.unwrap();

    assert_eq!(
        author_photo_source(&pool, canonical).await.as_deref(),
        Some("letter"),
        "undo must restore the canonical's own pre-merge photo"
    );
    let (restored_id, _) = author_row(&pool, "Manual Source").await.unwrap();
    assert_eq!(
        author_photo_source(&pool, restored_id).await.as_deref(),
        Some("manual"),
        "the recreated source's photo must follow it"
    );
}

#[tokio::test]
async fn apply_merge_authors_returns_not_found_for_unknown_source() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Canonical", None).await;
    let err = apply_merge_authors(&pool, &[9999], canonical, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::NotFound(9999)));
}

#[tokio::test]
async fn apply_merge_authors_returns_empty_sources_for_an_empty_list() {
    let pool = new_pool().await;
    let canonical = insert_author(&pool, "Canonical", None).await;
    let err = apply_merge_authors(&pool, &[], canonical, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::EmptySources));
}

#[tokio::test]
async fn apply_merge_authors_returns_canonical_is_source_when_they_collide() {
    let pool = new_pool().await;
    let a = insert_author(&pool, "A", None).await;
    let err = apply_merge_authors(&pool, &[a], a, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::CanonicalIsSource(id) if id == a));
}

// ---------------------------------------------------------------------------
// apply_merge_series / apply_merge_tags
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_merge_series_moves_link_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_series(&pool, "Foundation Series").await;
    let canonical = insert_series(&pool, "The Foundation Series").await;
    link_series(&pool, book, source).await;

    let log_id = apply_merge_series(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM series").await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM books_series_link WHERE book = {book} AND series = {canonical}"
            )
        )
        .await,
        1
    );
    assert!(alias_canonical(&pool, "series", "Foundation Series")
        .await
        .is_some());

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM series").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_series_link WHERE book = {book}")
        )
        .await,
        1,
        "the book must be linked to exactly the recreated source, not the former canonical"
    );
    assert_eq!(
        alias_canonical(&pool, "series", "Foundation Series").await,
        None
    );
}

#[tokio::test]
async fn apply_merge_tags_moves_link_across_three_books_and_undo_restores_state() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book_a = insert_book(&pool, lib, "book-a", "A").await;
    let book_b = insert_book(&pool, lib, "book-b", "B").await;
    let source = insert_tag(&pool, "scifi").await;
    let canonical = insert_tag(&pool, "sci-fi").await;
    link_tag(&pool, book_a, source).await;
    link_tag(&pool, book_b, canonical).await;
    link_tag(&pool, book_b, source).await;

    let log_id = apply_merge_tags(&pool, &[source], canonical, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
    assert!(fts_tags(&pool, book_a).await.contains("sci-fi"));
    assert!(fts_tags(&pool, book_b).await.contains("sci-fi"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book_a}")
        )
        .await,
        1
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book_b}")
        )
        .await,
        2,
        "book_b's independent pre-merge tag link must survive the round trip"
    );
}

// ---------------------------------------------------------------------------
// apply_tag_split
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_tag_split_creates_atoms_and_undo_restores_the_source_tag() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let source = insert_tag(&pool, "scifi;fantasy").await;
    link_tag(&pool, book, source).await;

    let atoms = vec!["scifi".to_string(), "fantasy".to_string()];
    let log_id = apply_tag_split(&pool, source, ";", &atoms, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
        )
        .await,
        2
    );
    let fts = fts_tags(&pool, book).await;
    assert!(fts.contains("scifi") && fts.contains("fantasy"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM tags").await, 1);
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_tags_link WHERE book = {book}")
        )
        .await,
        1
    );
    assert!(fts_tags(&pool, book).await.contains("scifi;fantasy"));
}

#[tokio::test]
async fn apply_tag_split_returns_too_few_atoms_for_a_single_atom_list() {
    let pool = new_pool().await;
    let source = insert_tag(&pool, "solo").await;
    let err = apply_tag_split(&pool, source, ";", &["solo".to_string()], None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::TooFewAtoms));
}

// ---------------------------------------------------------------------------
// apply_book_title_override
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_book_title_override_sets_override_without_touching_books_title_and_undo_deletes_it()
{
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Maas, Sarah J - Throne of Glass").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();

    let log_id = apply_book_title_override(&pool, &uuid, "Throne of Glass", None, user)
        .await
        .unwrap();

    let stored_title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        stored_title, "Maas, Sarah J - Throne of Glass",
        "books.title must never be touched directly"
    );
    let (ov, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(ov.title.as_deref(), Some("Throne of Glass"));

    undo(&pool, log_id).await.unwrap();

    assert!(get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn apply_book_title_override_preserves_other_override_fields_and_undo_restores_them() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let user = seed_user(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Cruft Title (Annotated)").await;
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book)
        .fetch_one(&pool)
        .await
        .unwrap();
    let existing = MetadataOverrides {
        description: Some("An existing description.".to_string()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &existing, false, user)
        .await
        .unwrap();

    let log_id = apply_book_title_override(&pool, &uuid, "Cruft Title", None, user)
        .await
        .unwrap();

    let (ov, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(ov.title.as_deref(), Some("Cruft Title"));
    assert_eq!(ov.description.as_deref(), Some("An existing description."));

    undo(&pool, log_id).await.unwrap();

    let (restored, _) = get_metadata_overrides(&pool, &uuid).await.unwrap().unwrap();
    assert_eq!(restored.title, None);
    assert_eq!(
        restored.description.as_deref(),
        Some("An existing description.")
    );
}

// ---------------------------------------------------------------------------
// apply_delete_author
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_delete_author_blocklists_name_and_undo_restores_author() {
    let pool = new_pool().await;
    let lib = seed_root(&pool).await;
    let book = insert_book(&pool, lib, "book-1", "Title One").await;
    let author = insert_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]", None).await;
    link_author(&pool, book, author, 0).await;

    let log_id = apply_delete_author(&pool, author, None, None)
        .await
        .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 0);
    assert!(is_ignored_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]").await);
    assert!(!fts_authors(&pool, book).await.contains("calibre"));

    undo(&pool, log_id).await.unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM authors").await, 1);
    assert!(!is_ignored_author(&pool, "calibre (0.7.23) [http://calibre-ebook.com]").await);
    assert!(fts_authors(&pool, book).await.contains("calibre"));
}

#[tokio::test]
async fn apply_delete_author_returns_not_found_for_unknown_id() {
    let pool = new_pool().await;
    let err = apply_delete_author(&pool, 9999, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, CleanupApplyError::NotFound(9999)));
}

// ---------------------------------------------------------------------------
// undo error paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn undo_returns_log_not_found_for_an_unknown_id() {
    let pool = new_pool().await;
    let err = undo(&pool, 9999).await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::LogNotFound(9999)));
}

#[tokio::test]
async fn undo_returns_already_undone_on_a_second_call() {
    let pool = new_pool().await;
    let author = insert_author(&pool, "Solo Author", None).await;
    let log_id = apply_delete_author(&pool, author, None, None)
        .await
        .unwrap();

    undo(&pool, log_id).await.unwrap();
    let err = undo(&pool, log_id).await.unwrap_err();
    assert!(matches!(err, CleanupApplyError::AlreadyUndone));
}
