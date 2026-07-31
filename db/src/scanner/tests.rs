//! Unit tests for the filesystem scanner: empty/missing-path handling,
//! extension counting, and recursive directory walks.

use std::fs;

use super::*;
use crate::test_support::make_test_dir;

#[test]
fn list_files_with_no_path_returns_empty() {
    let result = list_files(None, EBOOK_EXTENSIONS);
    assert_eq!(result.path, None);
    assert_eq!(result.total_files, 0);
    assert!(result.counts_by_ext.is_empty());
    assert!(result.error.is_none());
}

#[test]
fn list_files_with_nonexistent_path_returns_error() {
    let result = list_files(
        Some("/definitely/does/not/exist/omnibus_test"),
        EBOOK_EXTENSIONS,
    );
    assert!(result.error.is_some());
    assert_eq!(result.total_files, 0);
}

#[test]
fn list_files_counts_by_extension() {
    let dir = make_test_dir("counts");
    fs::write(dir.join("a.epub"), b"").unwrap();
    fs::write(dir.join("b.epub"), b"").unwrap();
    fs::write(dir.join("c.pdf"), b"").unwrap();
    fs::write(dir.join("d.txt"), b"").unwrap();
    fs::write(dir.join("e.cbz"), b"").unwrap();
    let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
    fs::remove_dir_all(&dir).unwrap();
    assert!(result.error.is_none());
    assert_eq!(result.total_files, 5);
    assert_eq!(
        result.counts_by_ext,
        vec![
            ("epub".to_string(), 2),
            ("pdf".to_string(), 1),
            ("cbz".to_string(), 1)
        ]
    );
}

#[test]
fn list_files_recurses_into_subdirectories() {
    let dir = make_test_dir("recursive");
    fs::write(dir.join("top.epub"), b"").unwrap();
    let nested = dir.join("series").join("vol1");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("deep.epub"), b"").unwrap();
    fs::write(nested.join("cover.jpg"), b"").unwrap();
    let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(result.total_files, 3);
    assert_eq!(
        result.counts_by_ext,
        vec![
            ("epub".to_string(), 2),
            ("pdf".to_string(), 0),
            ("cbz".to_string(), 0)
        ]
    );
}

#[test]
fn list_files_extension_match_is_case_insensitive() {
    let dir = make_test_dir("case");
    fs::write(dir.join("A.EPUB"), b"").unwrap();
    fs::write(dir.join("b.Pdf"), b"").unwrap();
    fs::write(dir.join("c.CbZ"), b"").unwrap();
    let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(
        result.counts_by_ext,
        vec![
            ("epub".to_string(), 1),
            ("pdf".to_string(), 1),
            ("cbz".to_string(), 1)
        ]
    );
}

#[test]
fn list_files_returns_empty_section_when_path_is_none() {
    // Silent contract: `path = None` short-circuits to a default-shaped
    // section (no path, no counts, no error) without touching the FS.
    let result = list_files(None, EBOOK_EXTENSIONS);
    assert_eq!(result, LibrarySection::default());
    assert!(result.path.is_none());
    assert_eq!(result.total_files, 0);
    assert!(result.counts_by_ext.is_empty());
    assert!(result.error.is_none());
}

#[test]
fn list_files_returns_section_with_error_when_path_missing() {
    // Silent contract: a path that doesn't exist on disk still returns
    // an Ok-shaped `LibrarySection`; the failure is surfaced via
    // `error: Some("path not found: ...")`, never as a panic or Err.
    let missing = "/definitely/does/not/exist/omnibus_scanner_missing_dir";
    let result = list_files(Some(missing), EBOOK_EXTENSIONS);
    assert_eq!(result.path.as_deref(), Some(missing));
    assert_eq!(result.total_files, 0);
    // Counts slot is preallocated for each requested extension, all zero.
    assert_eq!(
        result.counts_by_ext,
        vec![
            ("epub".to_string(), 0),
            ("pdf".to_string(), 0),
            ("cbz".to_string(), 0)
        ],
    );
    let err = result.error.expect("missing path should populate error");
    assert!(
        err.contains("path not found"),
        "unexpected error message: {err}",
    );
    assert!(
        err.contains(missing),
        "error should include the path: {err}"
    );
}

#[cfg(unix)]
#[test]
fn list_files_surfaces_io_error_on_unreadable_dir() {
    // Silent contract: when `read_dir` fails mid-walk (e.g. the directory
    // exists but is not readable), the section is returned with
    // `error: Some("could not read directory: ...")` instead of panicking
    // or propagating an `Err`.
    use std::os::unix::fs::PermissionsExt;

    let tmp = tempfile::tempdir().expect("create tempdir");
    let dir = tmp.path();
    // Seed a file so the walk has something to enumerate if it ever
    // succeeded; the test only cares about the read_dir failure path.
    fs::write(dir.join("a.epub"), b"").expect("seed file");

    let original = fs::metadata(dir).expect("stat tempdir").permissions();
    fs::set_permissions(dir, fs::Permissions::from_mode(0o000)).expect("strip dir perms");

    // Self-skip when the chmod didn't actually deny access (e.g. running
    // as root in a container — root bypasses DAC perms). Restore perms
    // before returning so TempDir cleanup can proceed.
    if fs::read_dir(dir).is_ok() {
        let _ = fs::set_permissions(dir, original);
        eprintln!(
            "skipping list_files_surfaces_io_error_on_unreadable_dir: \
             process can still read mode-000 dir (likely running as root)",
        );
        return;
    }

    let result = list_files(Some(dir.to_str().unwrap()), EBOOK_EXTENSIONS);

    // Restore perms *before* assertions so a failed assert can't leak a
    // mode-000 dir that breaks TempDir's Drop.
    fs::set_permissions(dir, original).expect("restore dir perms");

    assert_eq!(result.path.as_deref(), dir.to_str());
    assert_eq!(result.total_files, 0);
    assert_eq!(
        result.counts_by_ext,
        vec![
            ("epub".to_string(), 0),
            ("pdf".to_string(), 0),
            ("cbz".to_string(), 0)
        ],
    );
    let err = result.error.expect("unreadable dir should populate error");
    assert!(
        err.contains("could not read directory"),
        "unexpected error message: {err}",
    );
}

#[test]
fn scan_libraries_uses_audiobook_extensions() {
    let dir = make_test_dir("audiobooks");
    fs::write(dir.join("chapter1.m4b"), b"").unwrap();
    fs::write(dir.join("chapter2.mp3"), b"").unwrap();
    fs::write(dir.join("chapter3.mp3"), b"").unwrap();
    let path = dir.to_str().unwrap();
    let result = scan_libraries(None, Some(path));
    fs::remove_dir_all(&dir).unwrap();
    assert!(result.ebooks.path.is_none());
    assert_eq!(result.audiobooks.total_files, 3);
    assert_eq!(
        result.audiobooks.counts_by_ext,
        vec![("m4b".to_string(), 1), ("mp3".to_string(), 2)]
    );
}
