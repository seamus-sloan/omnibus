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
    /// True when the walk could not fully enumerate the tree — a subdir
    /// `read_dir` failed (EACCES, transient I/O), so `entries` is a
    /// **partial** view. The indexer must not run its removal pass on a
    /// partial enumeration, or it flags every un-enumerated book missing
    /// (see `crate::indexer::reindex`).
    pub incomplete: bool,
    /// True when the walk saw **any** regular file, of any extension. A
    /// populated root that reads *totally* empty (no files at all) is the
    /// boot-race / unmounted-NFS signature the indexer distrusts — as
    /// opposed to a shared root that legitimately has files of another
    /// format but no `.epub` (which stays trustworthy).
    pub saw_any_file: bool,
}

/// Phase A: walk the library and `stat` every indexed ebook file (`.epub`
/// / `.cbz`) without opening the zip. Returns one `StatEntry` per file
/// plus a synthetic entry (empty `uuid`) for any unreadable subdirectory —
/// the legacy `scan_ebook_library_with` shape surfaces these as error rows.
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
            incomplete: false,
            saw_any_file: false,
        };
    };

    let dir = Path::new(path_str);
    if !dir.exists() {
        return StatScanResult {
            path: Some(path_str.to_string()),
            entries: vec![],
            error: Some(format!("path not found: {path_str}")),
            incomplete: false,
            saw_any_file: false,
        };
    }

    let mut entries: Vec<StatEntry> = Vec::new();
    // Set when any subdir read fails: the enumeration is partial, so the
    // indexer must skip its removal pass rather than flag the missing
    // subtree as gone.
    let mut incomplete = false;
    // Set when the walk sees any regular file at all (any extension) — a
    // totally-empty read of a populated library is the transient-fault
    // signature the indexer distrusts.
    let mut saw_any_file = false;
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
                        incomplete: true,
                        saw_any_file: false,
                    };
                }
                incomplete = true;
                entries.push(unreadable_subdir_entry(dir, &current, &e));
                continue;
            }
        };
        // Iterate the ReadDir results explicitly rather than `.flatten()`:
        // an `Err` mid-enumeration is a partial `readdir` (an I/O fault
        // after the dir opened), so it must flag the walk `incomplete` — a
        // `.flatten()` here would silently drop the bad entry and leave the
        // enumeration looking complete, defeating the #819 removal guard.
        for entry in read {
            match entry {
                Ok(e) => push_dir_entry(dir, &e, &mut stack, &mut entries, &mut saw_any_file),
                Err(_) => incomplete = true,
            }
        }
    }

    entries.sort_by(|a, b| a.filename.cmp(&b.filename));

    StatScanResult {
        path: Some(path_str.to_string()),
        entries,
        error: None,
        incomplete,
        saw_any_file,
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
/// walk, and append a stat row for each indexed ebook file (an extension in
/// [`crate::ebook::EBOOK_FORMATS`] — `.epub` / `.cbz`). Other files and
/// entries whose `file_type()` can't be read are skipped. `saw_any_file` is
/// set for every regular file regardless of extension (the "root isn't
/// empty" signal, see the caller). Subdirectories matching
/// [`crate::helpers::is_skipped_scan_dir`] are not descended into.
fn push_dir_entry(
    base: &Path,
    entry: &std::fs::DirEntry,
    stack: &mut Vec<PathBuf>,
    entries: &mut Vec<StatEntry>,
    saw_any_file: &mut bool,
) {
    let Ok(file_type) = entry.file_type() else {
        return;
    };
    let entry_path = entry.path();
    if file_type.is_dir() {
        // Skipped at discovery, so an unreadable one never sets `incomplete`.
        if crate::helpers::is_skipped_scan_dir(&entry.file_name().to_string_lossy()) {
            return;
        }
        stack.push(entry_path);
        return;
    }
    if !file_type.is_file() {
        return;
    }
    *saw_any_file = true;
    // Keyed off EBOOK_FORMATS so the walk picks up exactly the formats the
    // reindex diff scopes to — the two can never drift apart.
    let is_indexed_ebook = entry_path
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| {
            crate::ebook::EBOOK_FORMATS
                .iter()
                .any(|f| s.eq_ignore_ascii_case(f))
        });
    if !is_indexed_ebook {
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
