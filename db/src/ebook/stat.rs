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
/// An entry with an empty `scan_key` is a synthetic placeholder for an
/// unreadable subdirectory — `error` carries the underlying io message
/// (e.g. "permission denied" vs. "no such file") so the legacy
/// [`super::scan_ebook_library_with`] wrapper can surface it verbatim. The
/// incremental diff ignores empty-`scan_key` entries.
#[derive(Debug, Clone, PartialEq)]
pub struct StatEntry {
    /// Path relative to the library root — same shape used everywhere else
    /// as the per-book "filename" string.
    pub filename: String,
    /// The Phase-A diff key (F2): the book's path *relative to the scan
    /// root*. Stored verbatim in `books.scan_key` so a library-root repoint
    /// — which leaves relative paths unchanged — preserves every
    /// `books.uuid`. Empty string for placeholder rows (see struct doc).
    pub scan_key: String,
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
/// `library_path_key` is retained for signature compatibility with the
/// audiobook walker; the diff key is now the library-relative path (F2), so
/// it no longer participates in identity. Empty-`scan_key` placeholder rows
/// mark unreadable subdirs for the legacy wrapper.
pub fn stat_ebook_library(path: Option<&str>, library_path_key: &str) -> StatScanResult {
    let _ = library_path_key;
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
                entries.push(unreadable_subdir_entry(dir, &current, &e));
                continue;
            }
        };
        for entry in read.flatten() {
            push_dir_entry(dir, &entry, &mut stack, &mut entries);
        }
    }

    entries.sort_by(|a, b| a.filename.cmp(&b.filename));

    StatScanResult {
        path: Some(path_str.to_string()),
        entries,
        error: None,
    }
}

/// Build the empty-`scan_key` placeholder for an unreadable subdirectory.
///
/// Carries the io::Error string so callers can distinguish "permission
/// denied" from "no such file" — the legacy wrapper lifts these into error
/// rows and the incremental indexer ignores empty-`scan_key` entries.
fn unreadable_subdir_entry(base: &Path, current: &Path, err: &std::io::Error) -> StatEntry {
    let relative = current
        .strip_prefix(base)
        .unwrap_or(current)
        .to_string_lossy()
        .to_string();
    StatEntry {
        filename: relative,
        scan_key: String::new(),
        mtime_epoch: 0,
        size_bytes: 0,
        error: Some(err.to_string()),
    }
}

/// Process one `read_dir` entry: push subdirectories onto `stack` for the
/// walk, and append a stat row for each `.epub` file. Non-epub files and
/// entries whose `file_type()` can't be read are skipped.
fn push_dir_entry(
    base: &Path,
    entry: &std::fs::DirEntry,
    stack: &mut Vec<PathBuf>,
    entries: &mut Vec<StatEntry>,
) {
    let Ok(file_type) = entry.file_type() else {
        return;
    };
    let entry_path = entry.path();
    if file_type.is_dir() {
        stack.push(entry_path);
        return;
    }
    if !file_type.is_file() {
        return;
    }
    let is_epub = entry_path
        .extension()
        .and_then(|s| s.to_str())
        .map(|s| s.eq_ignore_ascii_case("epub"))
        .unwrap_or(false);
    if !is_epub {
        return;
    }
    let relative = entry_path
        .strip_prefix(base)
        .unwrap_or(&entry_path)
        .to_string_lossy()
        .to_string();
    let (mtime_epoch, size_bytes) = stat_file(&entry_path);
    let scan_key = crate::helpers::scan_key_for(&relative);
    entries.push(StatEntry {
        filename: relative,
        scan_key,
        mtime_epoch,
        size_bytes,
        error: None,
    });
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
mod tests;
