//! Group per-file [`AudiobookStatEntry`] rows into one [`AudiobookGroup`].
//!
//! A single `.m4b`/`.m4a` file is its own book; a folder of per-chapter
//! `.mp3` files becomes one book per directory. Multi-file parts are
//! combined deliberately via the manual merge dialog, not by filename.

use super::stat::AudiobookStatEntry;
use crate::helpers::scan_key_for;

/// One audiobook entity post-grouping. The aggregate `mtime_epoch` /
/// `size_bytes` are used by the incremental diff to detect that *any*
/// part changed without re-walking each row — sum-of-sizes plus max-of-
/// mtimes is enough for invalidation purposes (worst case: same total
/// size after a rename, but mtime moves with any write).
#[derive(Debug, Clone, PartialEq)]
pub struct AudiobookGroup {
    /// Library-relative path identifying the group. For multi-mp3 groups
    /// this is the parent directory's relative path
    /// (e.g. `Author/Series/Title`); for single-file m4b/m4a/mp3 groups
    /// this is the file's own relative path including extension.
    pub group_path: String,
    /// The Phase-A diff key (F2): the group's library-relative path (same
    /// value as `group_path`). Empty for synthetic placeholder rows. Same
    /// role as [`crate::ebook::stat::StatEntry::scan_key`].
    pub scan_key: String,
    /// Per-part stat rows in the order returned by the walk. Phase B
    /// re-sorts by ID3 `track` tag; this ordering is only here so the
    /// diff result is reproducible.
    pub parts: Vec<AudiobookStatEntry>,
    /// Sum across `parts`.
    pub total_size_bytes: i64,
    /// Max across `parts` — any single part write bumps this, which is
    /// exactly what the diff needs.
    pub max_mtime_epoch: i64,
    /// Dominant source extension (uppercased — `M4B` / `M4A` / `MP3`).
    /// Drives the badge in `format_switcher` and `book_files.format`.
    pub format: String,
}

/// Group per-file stat entries into audiobook groups. Each `.m4b`/`.m4a`
/// file becomes its own one-file group; `.mp3` files bucket by parent
/// directory. Synthetic `error`-bearing entries from
/// `stat_audiobook_library` (empty uuid) pass through untouched in their
/// own one-part group so the legacy error-row contract is preserved at
/// the diff layer.
pub fn group_into_books(
    entries: Vec<AudiobookStatEntry>,
    library_path_key: &str,
) -> Vec<AudiobookGroup> {
    let mut singles: Vec<AudiobookGroup> = Vec::new();
    let mut mp3_buckets: std::collections::BTreeMap<String, Vec<AudiobookStatEntry>> =
        std::collections::BTreeMap::new();

    for entry in entries {
        if entry.scan_key.is_empty() {
            // Synthetic "unreadable subdir" placeholder. Pass through.
            singles.push(single(entry, "MP3", library_path_key));
            continue;
        }
        let ext = extension_of(&entry.filename).to_ascii_uppercase();
        match ext.as_str() {
            // Each m4b/m4a stands alone — the indexer no longer infers
            // multi-part bundles from filenames. Users combine parts
            // deliberately via the F5.10 manual merge dialog.
            "M4B" | "M4A" => singles.push(single(entry, &ext, library_path_key)),
            "MP3" => {
                let dir = parent_dir(&entry.filename).to_string();
                mp3_buckets.entry(dir).or_default().push(entry);
            }
            // Anything else slipped past the stat-extension filter — emit
            // a single-entry group with the raw extension so the format
            // badge still reads sensibly. In practice this is unreachable
            // (the stat walker only accepts `AUDIOBOOK_EXTENSIONS`), but
            // a defensive single-row fallback keeps the diff total honest
            // if a future extension lands in the walker without here.
            _ => singles.push(single(entry, &ext, library_path_key)),
        }
    }

    let mut groups = singles;
    for (dir, parts) in mp3_buckets {
        let total_size_bytes = parts.iter().map(|p| p.size_bytes).sum();
        let max_mtime_epoch = parts.iter().map(|p| p.mtime_epoch).max().unwrap_or(0);
        let scan_key = scan_key_for(&dir);
        groups.push(AudiobookGroup {
            group_path: dir,
            scan_key,
            parts,
            total_size_bytes,
            max_mtime_epoch,
            format: "MP3".into(),
        });
    }

    groups.sort_by(|a, b| a.group_path.cmp(&b.group_path));
    groups
}

fn single(entry: AudiobookStatEntry, format: &str, _library_path_key: &str) -> AudiobookGroup {
    // For single-file groups the canonical key is the file path itself,
    // not its parent dir — two `Author/Book.m4b` and
    // `Author/Bonus.m4b` files in the same directory must produce two
    // groups, not one.
    let group_path = entry.filename.clone();
    // The synthetic-error case carries an empty scan_key through; derive
    // here from the group key so downstream code (`diff_library`) sees a
    // proper key when the row is real, and an empty one when it's a
    // placeholder.
    let scan_key = if entry.scan_key.is_empty() {
        String::new()
    } else {
        scan_key_for(&group_path)
    };
    let size = entry.size_bytes;
    let mtime = entry.mtime_epoch;
    AudiobookGroup {
        group_path,
        scan_key,
        parts: vec![entry],
        total_size_bytes: size,
        max_mtime_epoch: mtime,
        format: format.into(),
    }
}

fn extension_of(filename: &str) -> &str {
    std::path::Path::new(filename)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
}

fn parent_dir(filename: &str) -> &str {
    std::path::Path::new(filename)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
}

#[cfg(test)]
mod tests;
