use crate::ebook::scan_ebook_library;
use crate::ebook::test_support::*;

#[test]
fn scan_with_no_path_returns_empty() {
    let out = scan_ebook_library(None);
    assert!(out.books.is_empty());
    assert!(out.path.is_none());
}

#[test]
fn scan_with_missing_path_reports_error() {
    let out = scan_ebook_library(Some("/definitely/does/not/exist/omnibus_ebook_test"));
    assert!(out.error.is_some());
}

#[test]
fn scan_ignores_non_epub_files() {
    let dir = make_test_dir("ignore");
    std::fs::write(dir.join("notes.txt"), b"hi").unwrap();
    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(out.books.is_empty());
    assert!(out.error.is_none());
}

#[test]
fn scan_recurses_into_subdirectories() {
    let dir = make_test_dir("recursive");
    let nested = dir.join("series").join("vol1");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(dir.join("top.epub"), b"not a zip").unwrap();
    std::fs::write(nested.join("deep.epub"), b"not a zip").unwrap();
    std::fs::write(nested.join("cover.jpg"), b"ignore").unwrap();
    let out = scan_ebook_library(Some(dir.to_str().unwrap()));
    std::fs::remove_dir_all(&dir).unwrap();
    // Two epubs + zero synthetic dir errors under a healthy tree.
    let book_entries: Vec<&str> = out
        .books
        .iter()
        .filter(|b| b.metadata.error.is_none() || !b.metadata.filename.is_empty())
        .map(|b| b.metadata.filename.as_str())
        .collect();
    assert_eq!(out.books.len(), 2);
    assert!(book_entries.contains(&"top.epub"));
    assert!(book_entries
        .iter()
        .any(|n| n.ends_with("deep.epub") && n.contains("series")));
}

// Permission tests only run where we can reliably strip read access
// from a directory — on Windows, even removing all permission bits
// doesn't reliably make `read_dir` fail as the owning process.
#[cfg(unix)]
#[test]
fn scan_continues_past_unreadable_subdirectory() {
    use std::os::unix::fs::PermissionsExt;

    let dir = make_test_dir("unreadable_subdir");
    std::fs::write(dir.join("good.epub"), b"not a zip").unwrap();
    let locked = dir.join("locked");
    std::fs::create_dir_all(&locked).unwrap();
    // Drop all permissions so read_dir on the subdir fails. Skip the
    // test on platforms where this doesn't take effect (e.g. running
    // as root in CI containers) rather than asserting a false
    // positive.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&locked).is_ok() {
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        return;
    }

    let out = scan_ebook_library(Some(dir.to_str().unwrap()));

    // Restore permissions before removing the tree, otherwise cleanup
    // fails on some platforms.
    std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();

    // Top-level scan must not report a fatal error.
    assert!(out.error.is_none());
    // Good epub still surfaces.
    assert!(out.books.iter().any(|b| b.metadata.filename == "good.epub"));
    // Locked subdir surfaces as a synthetic error entry, not silently
    // dropped, and exactly once — Phase B must skip the empty-uuid
    // placeholder so it doesn't get parsed as a fake epub AND appended
    // as an error row (which would produce two rows for the same dir).
    let locked_rows: Vec<_> = out
        .books
        .iter()
        .filter(|b| b.metadata.filename == "locked")
        .collect();
    assert_eq!(
        locked_rows.len(),
        1,
        "exactly one error row per unreadable subdir, got {locked_rows:?}",
    );
    let msg = locked_rows[0].metadata.error.as_deref().unwrap_or("");
    assert!(
        msg.contains("could not read directory"),
        "error message should keep the legacy prefix, got {msg:?}",
    );
    // Underlying io::Error detail (e.g. "permission denied") must be
    // carried through so callers can distinguish failure modes.
    assert!(
        msg.len() > "could not read directory".len(),
        "error message should include the io detail, got {msg:?}",
    );
}
