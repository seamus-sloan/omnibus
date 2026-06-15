//! Phase A walk for audiobook libraries — `stat` every accepted file
//! (`m4b` / `m4a` / `mp3`) without reading container tags. Mirrors
//! [`crate::ebook::stat`] one-to-one; consolidating into a single generic
//! walker would help here, but keeping the two side by side avoids
//! threading an extension list through the ebook path until a third
//! library type lands.

use std::path::{Path, PathBuf};

/// Phase A output: filesystem stat for one accepted audiobook file. The
/// shape matches [`crate::ebook::stat::StatEntry`] so the diff classifier
/// can be reused unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct AudiobookStatEntry {
    pub filename: String,
    /// The Phase-A diff key (F2): the file's path relative to the scan
    /// root. Empty for unreadable-subdir placeholder rows. Grouped into an
    /// [`super::AudiobookGroup`] scan_key downstream.
    pub scan_key: String,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
    pub error: Option<String>,
}

/// Phase A result. Mirrors `StatScanResult` for the no-path /
/// unreadable-root failure modes.
pub struct AudiobookStatScanResult {
    pub path: Option<String>,
    pub entries: Vec<AudiobookStatEntry>,
    pub error: Option<String>,
}

/// Walk `path`, stat every file with an extension in
/// [`super::AUDIOBOOK_EXTENSIONS`], and return one
/// [`AudiobookStatEntry`] per file. Synthetic placeholder rows for
/// unreadable subdirectories use the same empty-uuid convention as
/// [`crate::ebook::stat`], so the diff classifier ignores them.
pub fn stat_audiobook_library(
    path: Option<&str>,
    library_path_key: &str,
) -> AudiobookStatScanResult {
    // `library_path_key` no longer participates in identity (F2 keys the
    // diff on the relative path); retained for signature symmetry.
    let _ = library_path_key;
    let Some(path_str) = path else {
        return AudiobookStatScanResult {
            path: None,
            entries: vec![],
            error: None,
        };
    };

    let dir = Path::new(path_str);
    if !dir.exists() {
        return AudiobookStatScanResult {
            path: Some(path_str.to_string()),
            entries: vec![],
            error: Some(format!("path not found: {path_str}")),
        };
    }

    let mut entries: Vec<AudiobookStatEntry> = Vec::new();
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let read = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(e) => {
                if current == dir {
                    return AudiobookStatScanResult {
                        path: Some(path_str.to_string()),
                        entries: vec![],
                        error: Some(format!("could not read directory: {e}")),
                    };
                }
                let relative = current
                    .strip_prefix(dir)
                    .unwrap_or(&current)
                    .to_string_lossy()
                    .to_string();
                entries.push(AudiobookStatEntry {
                    filename: relative,
                    scan_key: String::new(),
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
            let ext = entry_path
                .extension()
                .and_then(|s| s.to_str())
                .map(str::to_ascii_lowercase);
            let accepted = ext
                .as_deref()
                .is_some_and(|e| super::AUDIOBOOK_EXTENSIONS.contains(&e));
            if !accepted {
                continue;
            }
            let relative = entry_path
                .strip_prefix(dir)
                .unwrap_or(&entry_path)
                .to_string_lossy()
                .to_string();
            let (mtime_epoch, size_bytes) = stat_file(&entry_path);
            let scan_key = crate::helpers::scan_key_for(&relative);
            entries.push(AudiobookStatEntry {
                filename: relative,
                scan_key,
                mtime_epoch,
                size_bytes,
                error: None,
            });
        }
    }

    entries.sort_by(|a, b| a.filename.cmp(&b.filename));

    AudiobookStatScanResult {
        path: Some(path_str.to_string()),
        entries,
        error: None,
    }
}

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
