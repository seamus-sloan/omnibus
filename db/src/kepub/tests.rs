//! Unit tests for the KEPUB cache: `is_stale` mtime comparison,
//! `convert_book` conversion + idempotency + error variants, and binary
//! detection. Conversion tests point `OMNIBUS_KEPUBIFY_PATH` at a fake shell
//! script (copy input→output, count runs) so the real binary isn't needed.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sqlx::SqlitePool;

use super::*;
use crate::test_support::EnvVarGuard;

/// Point `OMNIBUS_DATA_DIR` at a fresh tempdir for the test's duration.
fn data_dir_guard() -> (EnvVarGuard, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    (guard, dir)
}

/// Write a fake `kepubify` at `path`: handles `--version`, otherwise mimics the
/// real `kepubify -o <out> -- <src>` invocation (args `$1=-o $2=<out> $3=-- $4=<src>`)
/// by copying `$4` → `$2`, and appends a line to `counter` so callers can count
/// runs.
fn write_fake_kepubify(path: &Path, counter: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'kepubify 0-fake'; exit 0; fi\n\
         echo x >> '{counter}'\n\
         cp \"$4\" \"$2\"\n",
        counter = counter.display(),
    );
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Seed one EPUB book whose `book_file_path` resolves to a real file on disk
/// under `lib`. Returns the new `books.id`.
async fn seed_epub_book(pool: &SqlitePool, lib: &Path) -> i64 {
    let lib_str = lib.to_string_lossy().to_string();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(&lib_str)
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(&lib_str)
        .fetch_one(pool)
        .await
        .unwrap();
    // Old `last_modified` so a freshly-written cache always reads as fresh.
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified)
         VALUES ('uuid-1', 'sub/book.epub', ?, 'sub', 'Title', '2000-01-01 00:00:00')",
    )
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'EPUB', 'book', 4, 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    // The real EPUB the fake kepubify will copy.
    std::fs::create_dir_all(lib.join("sub")).unwrap();
    std::fs::write(lib.join("sub").join("book.epub"), b"epub").unwrap();
    book_id
}

#[test]
fn kepub_path_is_book_id_under_data_dir() {
    let (_g, dir) = data_dir_guard();
    let expected = dir.path().join("kepub").join("42.kepub.epub");
    assert_eq!(kepub_path(42), expected);
}

#[test]
fn kepub_dir_prefers_explicit_override_over_data_dir() {
    let data = tempfile::tempdir().unwrap();
    let over = tempfile::tempdir().unwrap();
    // OMNIBUS_KEPUB_DIR wins even when OMNIBUS_DATA_DIR is set, and is used
    // verbatim (no `kepub` subdir appended).
    let _g = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(data.path().as_os_str()))
        .also_set_os("OMNIBUS_KEPUB_DIR", Some(over.path().as_os_str()));
    assert_eq!(kepub_dir(), over.path());
    assert_eq!(kepub_path(42), over.path().join("42.kepub.epub"));
}

#[test]
fn is_stale_returns_true_when_cache_missing() {
    let (_g, _dir) = data_dir_guard();
    assert!(is_stale(999_999, 0));
}

#[test]
fn is_stale_returns_false_when_cache_newer_than_last_modified() {
    let (_g, dir) = data_dir_guard();
    std::fs::create_dir_all(dir.path().join("kepub")).unwrap();
    std::fs::write(kepub_path(1), b"cached").unwrap();
    let mtime = std::fs::metadata(kepub_path(1))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(!is_stale(1, mtime - 1));
}

#[test]
fn is_stale_returns_true_when_cache_older_than_last_modified() {
    let (_g, dir) = data_dir_guard();
    std::fs::create_dir_all(dir.path().join("kepub")).unwrap();
    std::fs::write(kepub_path(2), b"cached").unwrap();
    let mtime = std::fs::metadata(kepub_path(2))
        .unwrap()
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    assert!(is_stale(2, mtime + 100));
}

#[test]
fn kepubify_available_true_for_fake_binary_and_false_when_absent() {
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("kepubify");
    write_fake_kepubify(&script, &bin_dir.path().join("runs"));

    let guard = EnvVarGuard::set_os("OMNIBUS_KEPUBIFY_PATH", Some(script.as_os_str()));
    assert!(kepubify_available());
    drop(guard);

    let _absent = EnvVarGuard::set("OMNIBUS_KEPUBIFY_PATH", Some("/no/such/kepubify"));
    assert!(!kepubify_available());
}

#[tokio::test]
async fn convert_book_produces_cached_kepub_and_is_idempotent() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("kepubify");
    let counter = bin_dir.path().join("runs");
    write_fake_kepubify(&script, &counter);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_KEPUBIFY_PATH", Some(script.as_os_str()));

    let out = convert_book(&pool, book_id).await.unwrap();
    assert_eq!(out, kepub_path(book_id));
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"epub",
        "kepub is the converted bytes"
    );
    assert_eq!(run_count(&counter), 1);

    // Second call hits the fresh cache — kepubify does not run again.
    let out2 = convert_book(&pool, book_id).await.unwrap();
    assert_eq!(out2, out);
    assert_eq!(
        run_count(&counter),
        1,
        "cache hit must not re-spawn kepubify"
    );
}

#[tokio::test]
async fn convert_book_returns_book_not_found_for_unknown_id() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let (_env, _dir) = data_dir_guard();
    let err = convert_book(&pool, 424_242).await.unwrap_err();
    assert!(
        matches!(err, KepubError::BookNotFound(424_242)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn convert_book_returns_source_missing_when_book_has_no_epub() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    // Seed the book, then drop its book_files row so no EPUB resolves.
    let book_id = seed_epub_book(&pool, lib.path()).await;
    sqlx::query("DELETE FROM book_files WHERE book_id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    let (_env, _dir) = data_dir_guard();
    let err = convert_book(&pool, book_id).await.unwrap_err();
    assert!(
        matches!(err, KepubError::SourceMissing(id) if id == book_id),
        "got {err:?}"
    );
}

#[tokio::test]
async fn convert_book_errors_when_kepubify_binary_absent() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;
    let cache = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set("OMNIBUS_KEPUBIFY_PATH", Some("/no/such/kepubify"));
    let err = convert_book(&pool, book_id).await.unwrap_err();
    assert!(matches!(err, KepubError::Io(_)), "got {err:?}");
}

#[tokio::test]
async fn convert_book_returns_non_zero_when_kepubify_exits_non_zero() {
    // A kepubify run that exits non-zero (unsupported EPUB, internal error)
    // must surface as `KepubError::NonZero` carrying the exit status and the
    // captured stderr, so the caller can log it and fall back to plain EPUB —
    // never a torn cache file. A fake binary that prints to stderr and exits 3
    // drives the exit-code branch in `run_kepubify`.
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("kepubify");
    write_failing_kepubify(&script);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_KEPUBIFY_PATH", Some(script.as_os_str()));

    let err = convert_book(&pool, book_id).await.unwrap_err();
    assert!(
        matches!(&err, KepubError::NonZero { stderr, .. } if stderr.contains("boom")),
        "got {err:?}"
    );
    // No cache file left behind on failure.
    assert!(!kepub_path(book_id).exists());
}

/// Write a fake `kepubify` at `path` that answers `--version` (so detection
/// passes) but for any real invocation writes to stderr and exits non-zero,
/// driving the `NonZero` branch in `run_kepubify`.
fn write_failing_kepubify(path: &Path) {
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'kepubify 0-fake'; exit 0; fi\n\
         echo 'boom: conversion failed' 1>&2\n\
         exit 3\n";
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Number of lines the fake kepubify appended to `counter` (== run count).
fn run_count(counter: &Path) -> usize {
    std::fs::read_to_string(counter)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}
