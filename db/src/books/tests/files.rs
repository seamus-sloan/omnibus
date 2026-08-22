//! Resolving a book's files on disk: `book_file_path`,
//! `book_file_relative_dir`, the batched `book_file_paths`, and
//! `list_indexed_rows_for_formats`.

use crate::pool::init_db;

use super::super::*;

#[tokio::test]
async fn book_file_path_returns_absolute_path_for_epub() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    // `books.path` is stored RELATIVE to the library root (the scanner's
    // `root.join(filename)` convention), so the resolved path must be
    // `<libraries.path>/<books.path>/<stem>.<ext>`.
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-epub', ?, 'sub/dir', 'Some Book')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'some-book', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let path = book_file_path(&pool, book_id, "EPUB").await.unwrap();
    assert_eq!(
        path,
        Some(std::path::PathBuf::from("/lib/sub/dir/some-book.epub"))
    );
}

#[tokio::test]
async fn book_file_relative_dir_returns_library_relative_directory_for_epub() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-epub-rel', ?, 'sub/dir', 'Some Book')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'some-book', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let dir = book_file_relative_dir(&pool, book_id, "EPUB")
        .await
        .unwrap();
    // Relative to the scan root only — never includes `/lib`, unlike
    // `book_file_path`.
    assert_eq!(dir, Some(std::path::PathBuf::from("sub/dir")));
}

#[tokio::test]
async fn book_file_relative_dir_returns_none_for_missing_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = book_file_relative_dir(&pool, 9999, "EPUB").await.unwrap();
    assert!(dir.is_none());
}

#[tokio::test]
async fn book_file_path_returns_none_for_missing_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let path = book_file_path(&pool, 9999, "EPUB").await.unwrap();
    assert!(path.is_none());
}

#[tokio::test]
async fn book_file_path_returns_none_when_no_file_row_for_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-nofile', ?, '/lib/Bookless', 'Bookless')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();

    let path = book_file_path(&pool, book_id, "EPUB").await.unwrap();
    assert!(path.is_none());
}

#[tokio::test]
async fn book_file_paths_resolves_every_id_in_one_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_a = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-a', ?, 'sub/dir', 'Book A')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    let book_b = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('uuid-b', ?, 'other', 'Book B')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'book-a', 0)",
    )
    .bind(book_a)
    .execute(&pool)
    .await
    .unwrap();
    // book_b has two EPUB files; the lower ordinal must win, same tie-break
    // as `book_file_path`.
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes) \
         VALUES (?, 'EPUB', 'book-b-second', 1, 0)",
    )
    .bind(book_b)
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, ordinal, size_bytes) \
         VALUES (?, 'EPUB', 'book-b-first', 0, 0)",
    )
    .bind(book_b)
    .execute(&pool)
    .await
    .unwrap();

    let map = book_file_paths(&pool, &[book_a, book_b, 9999], "EPUB")
        .await
        .unwrap();

    assert_eq!(map.len(), 2, "the unknown id must be absent, got {map:?}");
    assert_eq!(
        map.get(&book_a),
        Some(&std::path::PathBuf::from("/lib/sub/dir/book-a.epub"))
    );
    assert_eq!(
        map.get(&book_b),
        Some(&std::path::PathBuf::from("/lib/other/book-b-first.epub")),
        "the lower-ordinal file must win"
    );
}

#[tokio::test]
async fn book_file_paths_returns_empty_map_for_empty_ids() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let map = book_file_paths(&pool, &[], "EPUB").await.unwrap();
    assert!(map.is_empty());
}

#[tokio::test]
async fn book_file_paths_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = book_file_paths(&pool, &[1], "EPUB").await.unwrap_err();
    assert!(matches!(err, BooksError::Db(_)), "got {err:?}");
}

// ---------- list_indexed_rows_for_formats (#328) ----------

#[tokio::test]
async fn list_indexed_rows_for_formats_returns_only_matching_format_rows() {
    // Regression for #328: when ebook and audiobook libraries share a
    // path, the format-scoped read must return only the rows whose
    // `book_files.format` is in the allow-list.
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Seed one EPUB and one M4B row under the same library_path. Use
    // separate library rows to keep the seed helper simple — they share
    // the same `libraries.path` string only via the second seed adding
    // its own row, so we instead insert both books under the same id.
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/shared', '/shared') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    // Both `books.scan_key` and its anchor `book_files.scan_key` are set to
    // the same value (matching what `insert_book_row` writes in production)
    // — that equality is what the per-file anchor match now keys on (#1537).
    let epub_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort) \
         VALUES ('uuid-epub', 'shared/epub/EpubTitle.epub', ?, \
                 '/shared/epub', 'EpubTitle', 'EpubTitle') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    let m4b_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort) \
         VALUES ('uuid-m4b', 'shared/audio/AudioTitle.m4b', ?, \
                 '/shared/audio', 'AudioTitle', 'AudioTitle') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key) \
         VALUES (?, 'EPUB', 'EpubTitle', 100, 100, 'shared/epub/EpubTitle.epub'), \
                (?, 'M4B',  'AudioTitle', 200, 200, 'shared/audio/AudioTitle.m4b')",
    )
    .bind(epub_id)
    .bind(m4b_id)
    .execute(&pool)
    .await
    .unwrap();

    let ebooks = list_indexed_rows_for_formats(&pool, "/shared", &["EPUB"])
        .await
        .unwrap();
    assert_eq!(ebooks.len(), 1);
    assert_eq!(ebooks[0].uuid, "uuid-epub");
    assert_eq!(ebooks[0].mtime_epoch, 100);
    assert_eq!(ebooks[0].size_bytes, 100);

    let audiobooks = list_indexed_rows_for_formats(&pool, "/shared", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    assert_eq!(audiobooks.len(), 1);
    assert_eq!(audiobooks[0].uuid, "uuid-m4b");
}

// ---------- migration 0024: drop dead book_files.mtime (F19) ----------

#[tokio::test]
async fn migration_drops_book_files_mtime_text_column_but_keeps_mtime_epoch() {
    // F19: the OPF `dcterms:modified` TEXT column was write-only and is
    // dropped by 0024; the filesystem-stat `mtime_epoch` (used by the
    // incremental reindex diff) must survive.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let columns: Vec<String> = sqlx::query_scalar(
        "SELECT name FROM pragma_table_info('book_files') WHERE name IN ('mtime', 'mtime_epoch')",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert!(
        !columns.iter().any(|c| c == "mtime"),
        "book_files.mtime should be dropped by migration 0024"
    );
    assert!(
        columns.iter().any(|c| c == "mtime_epoch"),
        "book_files.mtime_epoch must remain for change detection"
    );
}

#[tokio::test]
async fn list_indexed_rows_for_formats_returns_empty_for_empty_allow_list() {
    // Defensive contract: callers passing an empty allow-list mean
    // "no formats to match against" and must get an empty result, not
    // every row (which would re-introduce the #328 bug).
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', '/lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES ('uuid-a', ?, '/lib/a', 'A', 'A') RETURNING id",
    )
    .bind(lib_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, 'EPUB', 'a', 0, 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let rows = list_indexed_rows_for_formats(&pool, "/lib", &[])
        .await
        .unwrap();
    assert!(rows.is_empty());
}
