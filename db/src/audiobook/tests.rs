use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::parse::AudiobookError;
use super::*;

fn make_test_dir(suffix: &str) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omnibus_audiobook_{suffix}_{pid}_{seq}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("should create test dir");
    dir
}

#[test]
fn stat_with_no_path_returns_empty() {
    let out = stat_audiobook_library(None, "");
    assert!(out.entries.is_empty());
    assert!(out.path.is_none());
    assert!(out.error.is_none());
}

#[test]
fn stat_with_missing_path_reports_error() {
    let out = stat_audiobook_library(
        Some("/definitely/does/not/exist/omnibus_audiobook_test"),
        "/definitely/does/not/exist/omnibus_audiobook_test",
    );
    assert!(out.error.is_some());
    assert!(out.entries.is_empty());
}

#[test]
fn stat_picks_up_each_accepted_extension_and_skips_others() {
    let dir = make_test_dir("exts");
    fs::write(dir.join("a.m4b"), b"x").unwrap();
    fs::write(dir.join("b.m4a"), b"x").unwrap();
    fs::write(dir.join("c.mp3"), b"x").unwrap();
    fs::write(dir.join("d.txt"), b"x").unwrap();
    fs::write(dir.join("e.epub"), b"x").unwrap();
    let out = stat_audiobook_library(Some(dir.to_str().unwrap()), dir.to_str().unwrap());
    fs::remove_dir_all(&dir).unwrap();
    let names: Vec<_> = out.entries.iter().map(|e| e.filename.as_str()).collect();
    assert!(out.error.is_none());
    assert_eq!(names, vec!["a.m4b", "b.m4a", "c.mp3"]);
    // Same uuid scheme as the ebook path — non-empty + per-file stable.
    assert!(out.entries.iter().all(|e| !e.uuid.is_empty()));
}

#[test]
fn stat_recurses_into_subdirectories() {
    let dir = make_test_dir("nested");
    let nested = dir.join("author").join("title");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("book.m4b"), b"x").unwrap();
    let out = stat_audiobook_library(Some(dir.to_str().unwrap()), dir.to_str().unwrap());
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.entries.len(), 1);
    assert!(out.entries[0].filename.ends_with("book.m4b"));
    assert!(out.entries[0].filename.contains("author"));
}

#[test]
fn parse_unreadable_file_surfaces_as_error_metadata_row() {
    // Phase B's per-file failure shape: an audiobook that fails to parse
    // becomes an IndexedBook with `metadata.error = Some(...)`, never a
    // panic — same contract the EPUB path uses so one bad file doesn't
    // suppress the rest of the library.
    let dir = make_test_dir("badfile");
    let f = dir.join("broken.m4b");
    fs::write(&f, b"not actually an m4b").unwrap();
    let out = parse_audiobook_targets(vec![AudiobookParseTarget {
        filename: "broken.m4b".into(),
        absolute: f.clone(),
        mtime_epoch: 42,
        size_bytes: 19,
    }]);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.len(), 1);
    let row = &out[0];
    assert!(row.metadata.error.is_some());
    assert_eq!(row.metadata.filename, "broken.m4b");
    assert_eq!(row.mtime_epoch, 42);
    assert_eq!(row.size_bytes, 19);
}

#[test]
fn audiobook_error_io_variant_renders_useful_message() {
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.m4b");
    let err: AudiobookError = io.into();
    let s = err.to_string();
    assert!(s.starts_with("io error"), "got {s:?}");
}

#[test]
fn audiobook_error_unsupported_variant_renders_format_token() {
    let err = AudiobookError::Unsupported("xyz".into());
    let s = err.to_string();
    assert!(s.contains("xyz"), "got {s:?}");
}
