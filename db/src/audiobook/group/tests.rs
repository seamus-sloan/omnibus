//! Tests for audiobook file grouping: which stat entries collapse into one
//! book (an mp3 folder, a lone m4b) versus stay separate (multiple m4bs
//! sharing a base stem, mixed m4b/m4a in one dir), and error passthrough.

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
