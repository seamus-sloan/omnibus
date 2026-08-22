//! Unit tests for `convert`: binary resolution (`ebook_convert_bin`,
//! `is_runnable`, `ebook_convert_available`), the cache-path layout and its
//! traversal guard, and the execute path's success, failure, and timeout arms.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use sqlx::SqlitePool;

use crate::test_support::EnvVarGuard;

use super::*;

#[test]
fn ebook_convert_bin_returns_the_env_override_when_set() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/opt/calibre/ebook-convert"),
    );
    assert_eq!(ebook_convert_bin(), "/opt/calibre/ebook-convert");
}

#[test]
fn ebook_convert_bin_falls_back_to_the_bare_name_when_the_env_var_is_unset() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", None);
    assert_eq!(ebook_convert_bin(), "ebook-convert");
}

#[test]
fn ebook_convert_bin_treats_a_blank_env_override_as_unset() {
    let _guard = EnvVarGuard::set("OMNIBUS_EBOOK_CONVERT_PATH", Some("   "));
    assert_eq!(ebook_convert_bin(), "ebook-convert");
}

#[test]
fn is_runnable_returns_false_for_a_binary_that_does_not_exist() {
    assert!(!is_runnable("/nonexistent/omnibus-ebook-convert-probe"));
}

#[test]
fn is_runnable_returns_true_for_a_binary_that_exits_zero() {
    // `env --version` exits 0 on every platform the server targets, so it
    // stands in for a working Calibre install without needing one.
    assert!(is_runnable("env"));
}

#[test]
fn ebook_convert_available_returns_false_when_the_configured_binary_is_missing() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );
    assert!(!ebook_convert_available());
}

#[test]
fn warn_if_unavailable_does_not_panic_when_the_binary_is_missing() {
    let _guard = EnvVarGuard::set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );
    warn_if_unavailable();
}

// ---------------------------------------------------------------------------
// fs: convert_dir / convert_path
// ---------------------------------------------------------------------------

#[test]
fn convert_path_is_book_id_and_lowercased_target_format_under_data_dir() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    let expected = dir.path().join("convert").join("42.mobi");
    assert_eq!(convert_path(42, "MOBI").unwrap(), expected);
}

#[test]
fn convert_dir_prefers_explicit_override_over_data_dir() {
    let data = tempfile::tempdir().unwrap();
    let over = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(data.path().as_os_str()))
        .also_set_os("OMNIBUS_CONVERT_DIR", Some(over.path().as_os_str()));
    assert_eq!(convert_dir(), over.path());
    assert_eq!(convert_path(1, "AZW3").unwrap(), over.path().join("1.azw3"));
}

#[test]
fn convert_path_rejects_a_traversal_payload_in_target_format() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    let err = convert_path(1, "../../etc/passwd").unwrap_err();
    assert!(
        matches!(&err, ConvertError::InvalidFormat { field } if *field == "target_format"),
        "got {err:?}"
    );
}

#[test]
fn convert_path_rejects_an_embedded_path_separator_in_target_format() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    let err = convert_path(1, "a/b").unwrap_err();
    assert!(
        matches!(&err, ConvertError::InvalidFormat { field } if *field == "target_format"),
        "got {err:?}"
    );
}

#[test]
fn convert_path_rejects_an_empty_target_format() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    assert!(matches!(
        convert_path(1, "").unwrap_err(),
        ConvertError::InvalidFormat {
            field: "target_format"
        }
    ));
}

#[test]
fn convert_path_rejects_a_target_format_over_the_length_cap() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    let too_long = "a".repeat(17);
    assert!(matches!(
        convert_path(1, &too_long).unwrap_err(),
        ConvertError::InvalidFormat {
            field: "target_format"
        }
    ));
}

#[test]
fn convert_path_accepts_every_legitimate_conversion_target_format() {
    let dir = tempfile::tempdir().unwrap();
    let _guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    for fmt in ["EPUB", "MOBI", "AZW3", "PDF", "TXT", "DOCX", "FB2", "epub"] {
        assert!(
            convert_path(1, fmt).is_ok(),
            "expected {fmt} to be accepted"
        );
    }
}

// ---------------------------------------------------------------------------
// execute: convert_book
// ---------------------------------------------------------------------------

/// Point `OMNIBUS_DATA_DIR` at a fresh tempdir for the test's duration.
fn data_dir_guard() -> (EnvVarGuard, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let guard = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()));
    (guard, dir)
}

/// Seed one EPUB book whose `book_file_path` resolves to a real file on disk
/// under `lib`. Returns the new `books.id`.
async fn seed_epub_book(pool: &SqlitePool, lib: &Path) -> i64 {
    crate::test_support::seed_epub_book_at(pool, lib).await.0
}

/// Write a fake `ebook-convert` at `path`: handles `--version`, otherwise
/// mimics the real `ebook-convert <src> <out>` positional invocation (args
/// `$1=<src> $2=<out>`) by copying `$1` → `$2`, and appends a line to
/// `counter` so callers can count runs.
fn write_fake_ebook_convert(path: &Path, counter: &Path) {
    let script = format!(
        "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         echo x >> '{counter}'\n\
         cp \"$1\" \"$2\"\n",
        counter = counter.display(),
    );
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Write a fake `ebook-convert` that answers `--version` (so the probe
/// passes) but exits non-zero with stderr on a real invocation.
fn write_failing_ebook_convert(path: &Path) {
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         echo 'boom: conversion failed' 1>&2\n\
         exit 3\n";
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Write a fake `ebook-convert` that answers `--version` immediately but
/// sleeps past any short test timeout on a real invocation.
fn write_slow_ebook_convert(path: &Path) {
    let script = "#!/bin/sh\n\
         if [ \"$1\" = \"--version\" ]; then echo 'ebook-convert 0-fake'; exit 0; fi\n\
         sleep 5\n\
         cp \"$1\" \"$2\"\n";
    std::fs::write(path, script).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// Number of lines the fake binary appended to `counter` (== run count).
fn run_count(counter: &Path) -> usize {
    std::fs::read_to_string(counter)
        .map(|s| s.lines().count())
        .unwrap_or(0)
}

#[tokio::test]
async fn convert_book_produces_the_converted_file_at_the_cache_path() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    let counter = bin_dir.path().join("runs");
    write_fake_ebook_convert(&script, &counter);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let out = convert_book(&pool, book_id, "EPUB", "MOBI").await.unwrap();
    assert_eq!(out, convert_path(book_id, "MOBI").unwrap());
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"epub",
        "converted file is the source bytes (fake binary just copies)"
    );
    assert_eq!(run_count(&counter), 1);
}

#[tokio::test]
async fn convert_book_returns_source_missing_when_book_has_no_source_format_file() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;
    let dir = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    write_fake_ebook_convert(&script, &bin_dir.path().join("runs"));
    // Single chained guard: `data_dir_guard()` plus a second, separate
    // `EnvVarGuard::set*` call would try to acquire `ENV_LOCK` twice on the
    // same thread — the guard holds the lock for its whole lifetime, not
    // just during `set`, so that would self-deadlock.
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    // The seeded book only has an EPUB file; ask to convert from MOBI, which
    // it doesn't have.
    let err = convert_book(&pool, book_id, "MOBI", "AZW3")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ConvertError::SourceMissing { book_id: id, format } if *id == book_id && format.as_str() == "MOBI"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn convert_book_returns_binary_unavailable_when_ebook_convert_is_missing() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;
    let dir = tempfile::tempdir().unwrap();
    // Single chained guard — see the comment on the sibling test above for
    // why a separate `data_dir_guard()` + second guard would self-deadlock.
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(dir.path().as_os_str())).also_set(
        "OMNIBUS_EBOOK_CONVERT_PATH",
        Some("/nonexistent/omnibus-ebook-convert-probe"),
    );

    let err = convert_book(&pool, book_id, "EPUB", "MOBI")
        .await
        .unwrap_err();
    assert!(
        matches!(err, ConvertError::BinaryUnavailable),
        "got {err:?}"
    );
}

/// End-to-end: `convert_book` rejects a path-traversal payload in
/// `target_format` before touching the DB, the binary probe, or the
/// filesystem — no seeded book or configured `ebook-convert` is needed to
/// observe the rejection.
#[tokio::test]
async fn convert_book_rejects_a_path_traversal_payload_in_target_format() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let (_env, _dir) = data_dir_guard();

    let err = convert_book(&pool, 1, "EPUB", "../../etc/passwd")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ConvertError::InvalidFormat { field } if *field == "target_format"),
        "got {err:?}"
    );
}

/// Same as above for `source_format` — defense in depth, even though today
/// `source_format` only reaches a parameterized DB query and never a
/// filesystem path.
#[tokio::test]
async fn convert_book_rejects_a_path_traversal_payload_in_source_format() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let (_env, _dir) = data_dir_guard();

    let err = convert_book(&pool, 1, "../../etc/passwd", "MOBI")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ConvertError::InvalidFormat { field } if *field == "source_format"),
        "got {err:?}"
    );
}

#[tokio::test]
async fn convert_book_returns_failed_when_ebook_convert_exits_non_zero() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    write_failing_ebook_convert(&script);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let err = convert_book(&pool, book_id, "EPUB", "MOBI")
        .await
        .unwrap_err();
    assert!(
        matches!(&err, ConvertError::Failed(e) if e.to_string().contains("boom")),
        "got {err:?}"
    );
    // No cache file left behind on failure.
    assert!(!convert_path(book_id, "MOBI").unwrap().exists());
}

#[tokio::test]
async fn convert_book_returns_timeout_when_the_job_exceeds_the_configured_watchdog() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    write_slow_ebook_convert(&script);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()))
        .also_set("OMNIBUS_CONVERT_TIMEOUT_SECS", Some("1"));

    let err = convert_book(&pool, book_id, "EPUB", "MOBI")
        .await
        .unwrap_err();
    assert!(matches!(err, ConvertError::Timeout(1)), "got {err:?}");
    // No cache file left behind on timeout.
    assert!(!convert_path(book_id, "MOBI").unwrap().exists());
}

#[tokio::test]
async fn convert_book_propagates_db_error_when_pool_is_closed() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let (_env, _dir) = data_dir_guard();
    // `convert_book` resolves the effective ebook-convert path first, which
    // reads `settings` — a closed pool surfaces as the collapsed `Failed`
    // variant.
    let err = convert_book(&pool, 1, "EPUB", "MOBI").await.unwrap_err();
    assert!(matches!(err, ConvertError::Failed(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// persist: converted output becomes a book_files row
// ---------------------------------------------------------------------------

#[tokio::test]
async fn convert_book_inserts_a_book_files_row_for_the_converted_format() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    let counter = bin_dir.path().join("runs");
    write_fake_ebook_convert(&script, &counter);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let out = convert_book(&pool, book_id, "EPUB", "MOBI").await.unwrap();

    let files = crate::books::get_book_files(&pool, book_id).await.unwrap();
    let mobi = files
        .iter()
        .find(|f| f.format.eq_ignore_ascii_case("MOBI"))
        .expect("converted MOBI row present");
    assert_eq!(
        mobi.size_bytes,
        std::fs::metadata(&out).unwrap().len() as i64
    );

    // AC2: both formats resolve to the same book identity and are listed
    // together, and the row's path reconstructs back to the real cache file.
    assert!(files.iter().any(|f| f.format.eq_ignore_ascii_case("EPUB")));
    let resolved = crate::book_file_path(&pool, book_id, "MOBI").await.unwrap();
    assert_eq!(resolved, Some(out));
}

#[tokio::test]
async fn reconverting_the_same_format_updates_the_existing_row_instead_of_duplicating() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    let counter = bin_dir.path().join("runs");
    write_fake_ebook_convert(&script, &counter);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    let first_out = convert_book(&pool, book_id, "EPUB", "MOBI").await.unwrap();
    // Re-run the same conversion; the second run must upsert the same
    // `book_files` row rather than insert a duplicate (AC3).
    let second_out = convert_book(&pool, book_id, "EPUB", "MOBI").await.unwrap();
    assert_eq!(second_out, first_out);
    assert_eq!(run_count(&counter), 2, "both runs actually re-converted");

    let files = crate::books::get_book_files(&pool, book_id).await.unwrap();
    let mobi_rows: Vec<_> = files
        .iter()
        .filter(|f| f.format.eq_ignore_ascii_case("MOBI"))
        .collect();
    assert_eq!(mobi_rows.len(), 1, "got {mobi_rows:?}");
}

#[tokio::test]
async fn converting_two_different_target_formats_produces_two_distinct_converted_rows() {
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    let lib = tempfile::tempdir().unwrap();
    let book_id = seed_epub_book(&pool, lib.path()).await;

    let cache = tempfile::tempdir().unwrap();
    let bin_dir = tempfile::tempdir().unwrap();
    let script = bin_dir.path().join("ebook-convert");
    let counter = bin_dir.path().join("runs");
    write_fake_ebook_convert(&script, &counter);
    let _env = EnvVarGuard::set_os("OMNIBUS_DATA_DIR", Some(cache.path().as_os_str()))
        .also_set_os("OMNIBUS_EBOOK_CONVERT_PATH", Some(script.as_os_str()));

    convert_book(&pool, book_id, "EPUB", "MOBI").await.unwrap();
    convert_book(&pool, book_id, "EPUB", "AZW3").await.unwrap();

    let files = crate::books::get_book_files(&pool, book_id).await.unwrap();
    assert!(files.iter().any(|f| f.format.eq_ignore_ascii_case("MOBI")));
    assert!(files.iter().any(|f| f.format.eq_ignore_ascii_case("AZW3")));
}
