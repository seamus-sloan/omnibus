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
mod tests {
    use super::*;
    use crate::audiobook::stat::AudiobookStatEntry;

    fn entry(name: &str, mtime: i64, size: i64) -> AudiobookStatEntry {
        AudiobookStatEntry {
            filename: name.into(),
            scan_key: crate::helpers::scan_key_for(name),
            mtime_epoch: mtime,
            size_bytes: size,
            error: None,
        }
    }

    #[test]
    fn single_m4b_at_depth_becomes_its_own_group() {
        let groups = group_into_books(
            vec![entry("Cait Jacobs/Princess Knight/pk.m4b", 100, 4000)],
            "/lib",
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_path, "Cait Jacobs/Princess Knight/pk.m4b");
        assert_eq!(groups[0].format, "M4B");
        assert_eq!(groups[0].parts.len(), 1);
        assert_eq!(groups[0].total_size_bytes, 4000);
        assert_eq!(groups[0].max_mtime_epoch, 100);
    }

    #[test]
    fn folder_of_mp3s_groups_by_parent_dir() {
        // Way of Kings shape: many per-chapter mp3s in one directory.
        let groups = group_into_books(
            vec![
                entry("Sanderson/Way of Kings/04 Chapter 1.mp3", 100, 1000),
                entry("Sanderson/Way of Kings/05 Chapter 2.mp3", 110, 1100),
                entry("Sanderson/Way of Kings/06 Chapter 3.mp3", 120, 1200),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.group_path, "Sanderson/Way of Kings");
        assert_eq!(g.format, "MP3");
        assert_eq!(g.parts.len(), 3);
        // Aggregates: sum-of-sizes, max-of-mtimes.
        assert_eq!(g.total_size_bytes, 3300);
        assert_eq!(g.max_mtime_epoch, 120);
        // scan_key sourced from the group's relative path, not from any
        // individual part — the diff matches on this across reindexes.
        assert_eq!(g.scan_key, "Sanderson/Way of Kings");
    }

    #[test]
    fn m4b_in_mp3_folder_breaks_into_two_groups() {
        // Mixed-format folder: a bundled m4b sibling next to chapter mp3s.
        // The m4b is treated as a whole-book file (its own group); the
        // mp3 siblings group together.
        let groups = group_into_books(
            vec![
                entry("Author/Title/all.m4b", 100, 5000),
                entry("Author/Title/01.mp3", 200, 1000),
                entry("Author/Title/02.mp3", 210, 1100),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 2);
        let m4b = groups.iter().find(|g| g.format == "M4B").unwrap();
        let mp3 = groups.iter().find(|g| g.format == "MP3").unwrap();
        assert_eq!(m4b.parts.len(), 1);
        assert_eq!(mp3.parts.len(), 2);
        assert_eq!(mp3.group_path, "Author/Title");
    }

    #[test]
    fn empty_input_yields_empty_groups() {
        let groups = group_into_books(vec![], "/lib");
        assert!(groups.is_empty());
    }

    #[test]
    fn synthetic_error_entry_passes_through() {
        // `stat_audiobook_library` emits an empty-uuid placeholder for
        // unreadable subdirectories. The diff treats those as
        // not-present, so the grouping pass must preserve the empty uuid
        // (not invent one) and pass the row through unchanged.
        let bad = AudiobookStatEntry {
            filename: "locked".into(),
            scan_key: String::new(),
            mtime_epoch: 0,
            size_bytes: 0,
            error: Some("permission denied".into()),
        };
        let groups = group_into_books(vec![bad.clone()], "/lib");
        assert_eq!(groups.len(), 1);
        assert!(groups[0].scan_key.is_empty());
        assert_eq!(groups[0].parts.len(), 1);
        assert_eq!(
            groups[0].parts[0].error.as_deref(),
            Some("permission denied")
        );
    }

    #[test]
    fn each_m4b_becomes_its_own_group_even_with_shared_base_stem() {
        // The Wind-and-Truth repro: 5 distinct M4B files for the same
        // book, named inconsistently (`(N of 5)` suffixes that strip to
        // the same base, plus a stray `[05]` style). Pre-fix, the
        // filename heuristic bucketed `(3 of 5)` and `(5 of 5)` into a
        // single book because their base stems collided after stripping
        // — silently hiding part 5 inside part 3's `book_file_parts`.
        // Post-fix every file stands alone; user combines them via the
        // manual merge dialog if they want one playable unit.
        let groups = group_into_books(
            vec![
                entry(
                    "Brandon Sanderson/Wind and Truth/Stormlight Archive [05] Wind and Truth (1 of 5).m4b",
                    100,
                    1000,
                ),
                entry(
                    "Brandon Sanderson/Wind and Truth/Stormlight Archive [05] Wind and Truth (2 of 5).m4b",
                    110,
                    1100,
                ),
                entry(
                    "Brandon Sanderson/Wind and Truth/Stormlight Archive [05] Wind and Truth (3 of 5).m4b",
                    120,
                    1200,
                ),
                entry(
                    "Brandon Sanderson/Wind and Truth/Stormlight Archive 05 Wind and Truth (4 of 5).m4b",
                    130,
                    1300,
                ),
                entry(
                    "Brandon Sanderson/Wind and Truth/Stormlight Archive [05] Wind and Truth (5 of 5).m4b",
                    140,
                    1400,
                ),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 5);
        for g in &groups {
            assert_eq!(g.format, "M4B");
            assert_eq!(g.parts.len(), 1);
        }
    }

    #[test]
    fn two_part_m4bs_in_one_dir_stay_separate_groups() {
        // Pre-fix: the indexer bucketed these into a single multi-part
        // group keyed on `Dracula Pt1.m4b`. Post-fix each file is its
        // own group — the user merges them deliberately if intended.
        let groups = group_into_books(
            vec![
                entry("Stoker/Dracula Pt2.m4b", 200, 2000),
                entry("Stoker/Dracula Pt1.m4b", 100, 1000),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 2);
        for g in &groups {
            assert_eq!(g.format, "M4B");
            assert_eq!(g.parts.len(), 1);
        }
    }

    #[test]
    fn m4b_and_m4a_in_same_dir_stay_separate_groups() {
        let groups = group_into_books(
            vec![
                entry("A/Book Pt1.m4b", 100, 1000),
                entry("A/Book Pt2.m4a", 110, 1100),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 2);
    }

    #[test]
    fn unsupported_extension_falls_back_to_single_group() {
        // Defensive: any extension that slipped past the walker still
        // produces a deterministic group rather than getting dropped.
        let groups = group_into_books(vec![entry("Author/Title/track.flac", 50, 999)], "/lib");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].format, "FLAC");
    }
}
