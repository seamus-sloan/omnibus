//! Tests for the content FTS index: extraction, the snapshot-keyed backfill
//! pass, pruning, and the bm25 content-search read path.

use std::path::Path;

use sqlx::SqlitePool;

use crate::ebook::test_support::copy_fixture_into;
use crate::pool::init_db;
use crate::test_support::{build_test_epub, count_rows, make_test_dir};

use super::*;

/// Seed one book backed by a real fixture EPUB on disk, with an explicit
/// `(mtime_epoch, size_bytes)` on its `book_files` row so the snapshot tests
/// can move the pair deliberately. Returns `books.id`.
async fn seed_epub_book(
    pool: &SqlitePool,
    dir: &Path,
    fixture_name: &str,
    uuid: &str,
    title: &str,
    mtime_epoch: i64,
    size_bytes: i64,
) -> i64 {
    copy_fixture_into(fixture_name, dir);
    seed_book_row(
        pool,
        dir,
        fixture_name,
        "EPUB",
        uuid,
        title,
        mtime_epoch,
        size_bytes,
    )
    .await
}

/// The DB half of [`seed_epub_book`] — no file copy, so it also serves the
/// missing-file and non-EPUB cases.
#[allow(clippy::too_many_arguments)]
async fn seed_book_row(
    pool: &SqlitePool,
    dir: &Path,
    file_name: &str,
    format: &str,
    uuid: &str,
    title: &str,
    mtime_epoch: i64,
    size_bytes: i64,
) -> i64 {
    let dir_str = dir.to_str().unwrap();
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(dir_str)
    .bind(dir_str)
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(dir_str)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    let stem = file_name.rsplit_once('.').map_or(file_name, |(s, _)| s);
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(book_id)
    .bind(format)
    .bind(stem)
    .bind(size_bytes)
    .bind(mtime_epoch)
    .execute(pool)
    .await
    .unwrap();
    book_id
}

async fn chapter_texts_of(pool: &SqlitePool, uuid: &str) -> Vec<String> {
    sqlx::query_scalar(
        "SELECT text FROM book_content_chapters WHERE book_uuid = ? ORDER BY spine_index",
    )
    .bind(uuid)
    .fetch_all(pool)
    .await
    .unwrap()
}

// ---------- extraction ----------

#[test]
fn extract_chapter_texts_strips_markup_and_keeps_spine_positions() {
    let dir = make_test_dir("content-extract");
    let epub = build_test_epub(&[
        (
            "c1.xhtml",
            "<html><head><style>p{color:red}</style></head>\
             <body><p>The moon rose over the harbor.</p></body></html>",
        ),
        ("c2.xhtml", "<html><body><p></p></body></html>"),
        (
            "c3.xhtml",
            "<html><body><p>A second chapter of prose.</p></body></html>",
        ),
    ]);
    let path = dir.join("book.epub");
    std::fs::write(&path, epub).unwrap();

    let chapters = extract_chapter_texts(&path).expect("epub should extract");
    let pairs: Vec<(i64, &str)> = chapters
        .iter()
        .map(|c| (c.spine_index, c.text.as_str()))
        .collect();
    // The empty chapter at spine 1 is dropped, and the survivors keep their
    // true spine positions; the stylesheet body never reaches the text.
    assert_eq!(
        pairs,
        vec![
            (0, "The moon rose over the harbor."),
            (2, "A second chapter of prose."),
        ]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extract_chapter_texts_returns_none_for_a_file_that_is_not_an_epub() {
    let dir = make_test_dir("content-extract-bad");
    let path = dir.join("bad.epub");
    std::fs::write(&path, b"not a zip").unwrap();
    assert!(extract_chapter_texts(&path).is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- backfill ----------

#[tokio::test]
async fn backfill_content_fts_indexes_fixture_epub_and_content_search_finds_body_phrase() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-backfill");
    let lib = dir.to_str().unwrap().to_string();
    let book_id = seed_epub_book(&pool, &dir, "alpha.epub", "uuid-a", "Alpha", 100, 200).await;
    // Give the metadata index its real row, so "the metadata search does not
    // find the body phrase" is a live assertion rather than an empty table.
    sqlx::query(
        "INSERT INTO books_fts (rowid, title, authors, series, tags, description, isbn, genres) \
         VALUES (?, 'Alpha', 'Ada Lovelace', '', '', '', '', '')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_content_fts(&pool, &lib, |p, t, _| progress.push((p, t)))
        .await
        .unwrap();
    assert_eq!(progress, vec![(1, 1)]);

    // alpha.epub's body phrase (AC1): found by the content search, with a
    // chapter citation (AC2)…
    let hits = search_content_for_paths(&pool, &[&lib], "Synthetic test content")
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].book_uuid, "uuid-a");
    assert_eq!(hits[0].spine_index, 0);
    assert_eq!(hits[0].title, "Alpha");
    assert!(
        hits[0].snippet.contains("[Synthetic]"),
        "snippet must mark the matched term: {}",
        hits[0].snippet
    );

    // …and not by the metadata search over the same library.
    let metadata_hits =
        crate::books::search_books_for_paths(&pool, &[&lib], "Synthetic test content")
            .await
            .unwrap();
    assert!(
        metadata_hits.is_empty(),
        "body text must not be reachable through books_fts"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_content_fts_skips_book_whose_snapshot_matches() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-skip");
    let lib = dir.to_str().unwrap().to_string();
    seed_epub_book(&pool, &dir, "alpha.epub", "uuid-a", "Alpha", 100, 200).await;
    backfill_content_fts(&pool, &lib, |_, _, _| {})
        .await
        .unwrap();

    // Tamper with the stored text; a second pass over an unchanged snapshot
    // must not touch it — if it re-extracted, the marker would be gone.
    sqlx::query("UPDATE book_content_chapters SET text = 'tamper-sentinel'")
        .execute(&pool)
        .await
        .unwrap();
    let mut calls = 0u32;
    backfill_content_fts(&pool, &lib, |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0, "an unchanged book must not be a candidate");
    assert_eq!(
        chapter_texts_of(&pool, "uuid-a").await,
        vec!["tamper-sentinel".to_string()]
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_content_fts_reindexes_book_when_snapshot_changes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-changed");
    let lib = dir.to_str().unwrap().to_string();
    seed_epub_book(&pool, &dir, "alpha.epub", "uuid-a", "Alpha", 100, 200).await;
    backfill_content_fts(&pool, &lib, |_, _, _| {})
        .await
        .unwrap();
    sqlx::query("UPDATE book_content_chapters SET text = 'tamper-sentinel'")
        .execute(&pool)
        .await
        .unwrap();

    // A Changed file lands as a new stat on the book_files row (AC4); the
    // stale rows must be replaced, not left.
    sqlx::query("UPDATE book_files SET mtime_epoch = 101")
        .execute(&pool)
        .await
        .unwrap();
    backfill_content_fts(&pool, &lib, |_, _, _| {})
        .await
        .unwrap();

    let texts = chapter_texts_of(&pool, "uuid-a").await;
    assert_eq!(texts.len(), 1, "delete + reinsert must not duplicate rows");
    assert!(texts[0].contains("Synthetic test content"));
    let stored_mtime: i64 =
        sqlx::query_scalar("SELECT DISTINCT mtime_epoch FROM book_content_chapters")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        stored_mtime, 101,
        "rows must carry the snapshot they were extracted at"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_content_fts_skips_formats_with_no_text_silently() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-noext");
    let lib = dir.to_str().unwrap().to_string();
    seed_book_row(&pool, &dir, "audio.m4b", "M4B", "uuid-m4b", "Spoken", 1, 1).await;

    let mut calls = 0u32;
    backfill_content_fts(&pool, &lib, |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0, "a non-EPUB book is never a candidate");
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_content_chapters").await,
        0
    );
}

#[tokio::test]
async fn backfill_content_fts_skips_unreadable_epub_without_failing_the_pass() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-unreadable");
    let lib = dir.to_str().unwrap().to_string();
    // book_files row with no file on disk behind it.
    seed_book_row(
        &pool,
        &dir,
        "ghost.epub",
        "EPUB",
        "uuid-ghost",
        "Ghost",
        5,
        5,
    )
    .await;

    backfill_content_fts(&pool, &lib, |_, _, _| {})
        .await
        .expect("an unreadable file must not error the pass (AC5)");
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_content_chapters").await,
        0
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_content_fts_prunes_rows_for_books_that_no_longer_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("content-prune");
    let lib = dir.to_str().unwrap().to_string();
    sqlx::query(
        "INSERT INTO book_content_chapters (book_uuid, spine_index, mtime_epoch, size_bytes, text) \
         VALUES ('uuid-orphan', 0, 1, 1, 'stranded text')",
    )
    .execute(&pool)
    .await
    .unwrap();

    backfill_content_fts(&pool, &lib, |_, _, _| {})
        .await
        .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_content_chapters").await,
        0,
        "rows whose uuid resolves to no book must be pruned"
    );
}

// ---------- search ----------

#[tokio::test]
async fn search_content_for_paths_returns_empty_for_empty_query_and_no_paths() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(search_content_for_paths(&pool, &["/lib"], "   ")
        .await
        .unwrap()
        .is_empty());
    assert!(search_content_for_paths(&pool, &[], "moon")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn search_content_for_paths_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = search_content_for_paths(&pool, &["/lib"], "moon")
        .await
        .expect_err("closed pool must surface as ContentFtsError::Db");
    assert!(matches!(err, ContentFtsError::Db(_)));
}
