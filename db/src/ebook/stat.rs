//! Phase A of the ebook scan: cheap filesystem stat per file.
//!
//! Emits one [`StatEntry`] per ebook file under the library root without
//! opening the zip or parsing the OPF. The incremental diff in
//! [`crate::ebook`] consumes these to classify entries against `book_files`.

use std::path::{Path, PathBuf};

/// Phase A output: a single filesystem stat row, with no zip open or OPF
/// parse. The incremental diff compares these against `book_files` to
/// classify entries as Unchanged / New / Changed / Removed / Backfill.
///
/// An entry with an empty `uuid` is a synthetic placeholder for an
/// unreadable subdirectory — `error` carries the underlying io message
/// (e.g. "permission denied" vs. "no such file") so the legacy
/// [`super::scan_ebook_library_with`] wrapper can surface it verbatim. The
/// incremental diff ignores empty-uuid entries.
#[derive(Debug, Clone, PartialEq)]
pub struct StatEntry {
    /// Path relative to the library root — same shape used everywhere else
    /// as the per-book "filename" string.
    pub filename: String,
    /// Stable UUIDv5 of `(library_path, filename)`. Computed here so the
    /// diff branch never needs to thread `library_path` around again.
    /// Empty string for placeholder rows (see struct doc).
    pub uuid: String,
    /// Filesystem mtime in seconds since the unix epoch, or `0` if
    /// `entry.metadata().modified()` failed.
    pub mtime_epoch: i64,
    /// Filesystem byte size, or `0` if stat failed.
    pub size_bytes: i64,
    /// Only populated for placeholder rows — the original io::Error
    /// message string from the failed `read_dir`. `None` for real epub
    /// entries.
    pub error: Option<String>,
}

/// Phase A result. Mirrors `ScanResult` for the no-path / unreadable-root
/// failure modes.
pub struct StatScanResult {
    pub path: Option<String>,
    pub entries: Vec<StatEntry>,
    pub error: Option<String>,
}

/// Phase A: walk the library and `stat` every `.epub` without opening the
/// zip. Returns one `StatEntry` per file plus a synthetic entry (empty
/// `uuid`) for any unreadable subdirectory — the legacy
/// `scan_ebook_library_with` shape surfaces these as error rows.
///
/// `library_path_key` is what `stable_uuid` hashes; callers pass the same
/// string they'd use as the `books.library_path` so the uuids line up with
/// any rows already in the DB.
pub fn stat_ebook_library(path: Option<&str>, library_path_key: &str) -> StatScanResult {
    let Some(path_str) = path else {
        return StatScanResult {
            path: None,
            entries: vec![],
            error: None,
        };
    };

    let dir = Path::new(path_str);
    if !dir.exists() {
        return StatScanResult {
            path: Some(path_str.to_string()),
            entries: vec![],
            error: Some(format!("path not found: {path_str}")),
        };
    }

    let mut entries: Vec<StatEntry> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let read = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(e) => {
                if current == dir {
                    // Root unreadable: fatal — match legacy behavior.
                    return StatScanResult {
                        path: Some(path_str.to_string()),
                        entries: vec![],
                        error: Some(format!("could not read directory: {e}")),
                    };
                }
                // Sub-dir unreadable: record a placeholder with an empty
                // uuid so the legacy wrapper can lift it into an error
                // row. Carry the io::Error string so callers can
                // distinguish "permission denied" from "no such file" —
                // the incremental indexer ignores empty-uuid entries.
                let relative = current
                    .strip_prefix(dir)
                    .unwrap_or(&current)
                    .to_string_lossy()
                    .to_string();
                entries.push(StatEntry {
                    filename: relative,
                    uuid: String::new(),
                    mtime_epoch: 0,
                    size_bytes: 0,
                    error: Some(e.to_string()),
                });
                continue;
            }
        };
        for entry in read.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let entry_path = entry.path();
            if file_type.is_dir() {
                stack.push(entry_path);
                continue;
            }
            if !file_type.is_file() {
                continue;
            }
            let is_epub = entry_path
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("epub"))
                .unwrap_or(false);
            if !is_epub {
                continue;
            }
            let relative = entry_path
                .strip_prefix(dir)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .to_string();
            let (mtime_epoch, size_bytes) = stat_file(&entry_path);
            let uuid = crate::helpers::stable_uuid(library_path_key, &relative);
            entries.push(StatEntry {
                filename: relative,
                uuid,
                mtime_epoch,
                size_bytes,
                error: None,
            });
        }
    }

    entries.sort_by(|a, b| a.filename.cmp(&b.filename));

    StatScanResult {
        path: Some(path_str.to_string()),
        entries,
        error: None,
    }
}

/// `entry.metadata()` + `modified()` → epoch seconds. Anything we can't
/// stat returns `(0, 0)`, which the diff treats as the "Backfill" sentinel
/// — same as a freshly migrated row. That's fine: if we couldn't stat it
/// now, the next run will either succeed (and the row updates) or keep
/// failing (and the row stays in the same Backfill-ish state). No data loss.
fn stat_file(path: &Path) -> (i64, i64) {
    let Ok(meta) = std::fs::metadata(path) else {
        return (0, 0);
    };
    let size = meta.len() as i64;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    (mtime, size)
}

#[cfg(test)]
mod tests {
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
}
