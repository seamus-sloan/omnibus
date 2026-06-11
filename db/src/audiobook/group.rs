//! Phase A.5 — group per-file [`AudiobookStatEntry`] rows into one
//! [`AudiobookGroup`] per audiobook.
//!
//! The shape of real audiobook libraries:
//!
//! * `Author/Book.m4b` — single file = one book. The `.m4b` (and `.m4a`)
//!   containers are inherently "the whole book" so each one is always its
//!   own group, even if other files share its parent directory.
//! * `Author/Book Pt1.m4b, Book Pt2.m4b` — long books split into parts.
//!   Same-directory m4b/m4a files whose stems match after stripping a
//!   part designator (`Pt1`, `Part 2`, `Disc 3`, `1 of 3`, `- 2`, …)
//!   group into one book; see [`strip_part_suffix`]. A lone part-suffixed
//!   file stays its own group with an unchanged uuid.
//! * `Author/Book/chapter01.mp3, chapter02.mp3, …` — folder of per-chapter
//!   mp3s = one book. Group key is the parent directory's relative path;
//!   parts are ordered by ID3 `track` in Phase B (filename here is just
//!   the tiebreaker for the stable_uuid input).
//!
//! Mixed-format folders fall out naturally: every `.m4b`/`.m4a` becomes
//! its own group (or joins its part-siblings), then any remaining `.mp3`
//! files in the same directory form a sibling group. The cases we've
//! observed in practice never mix the two, but the rule keeps the
//! grouping deterministic if they do.

use super::stat::AudiobookStatEntry;
use crate::helpers::stable_uuid;

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
    /// Deterministic UUIDv5 of `(library_path_key, group_path)`. Stays
    /// stable across reindexes — same shape as
    /// [`crate::ebook::stat::StatEntry::uuid`].
    pub uuid: String,
    /// Per-part stat rows in the order returned by the walk. Phase B
    /// re-sorts by ID3 `track` tag; this ordering is only here so the
    /// `stable_uuid` input is deterministic and the diff result is
    /// reproducible.
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

/// Group per-file stat entries into audiobook groups. Synthetic
/// `error`-bearing entries from `stat_audiobook_library` (empty uuid)
/// pass through untouched in their own one-part group so the legacy
/// error-row contract is preserved at the diff layer.
pub fn group_into_books(
    entries: Vec<AudiobookStatEntry>,
    library_path_key: &str,
) -> Vec<AudiobookGroup> {
    // Two passes: pull every `.m4b`/`.m4a` (and synthetic error rows) out
    // first — part-suffixed stems bucket with their same-directory
    // siblings, the rest become one-entry groups — then bucket the
    // remaining `.mp3` rows by parent directory.
    let mut singles: Vec<AudiobookGroup> = Vec::new();
    let mut mp3_buckets: std::collections::BTreeMap<String, Vec<AudiobookStatEntry>> =
        std::collections::BTreeMap::new();
    // Key: (parent_dir, lowercased stripped stem, extension). The
    // extension is part of the key so an m4b and an m4a never share a
    // group (book_files.format is single-valued per group).
    type PartKey = (String, String, String);
    let mut part_buckets: std::collections::BTreeMap<PartKey, Vec<(u32, AudiobookStatEntry)>> =
        std::collections::BTreeMap::new();

    for entry in entries {
        if entry.uuid.is_empty() {
            // Synthetic "unreadable subdir" placeholder. Pass through.
            singles.push(single(entry, "MP3", library_path_key));
            continue;
        }
        let ext = extension_of(&entry.filename).to_ascii_uppercase();
        match ext.as_str() {
            "M4B" | "M4A" => match strip_part_suffix(file_stem_of(&entry.filename)) {
                Some((base, part_no)) => {
                    let key = (
                        parent_dir(&entry.filename).to_string(),
                        base.to_ascii_lowercase(),
                        ext,
                    );
                    part_buckets.entry(key).or_default().push((part_no, entry));
                }
                None => singles.push(single(entry, &ext, library_path_key)),
            },
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
    for ((_, _, ext), mut numbered) in part_buckets {
        if numbered.len() == 1 {
            // A lone `Book Pt1.m4b` with no siblings stays a normal
            // single-file group — same group_path/uuid it had before
            // part-grouping existed, so nothing churns on reindex.
            let (_, entry) = numbered.remove(0);
            groups.push(single(entry, &ext, library_path_key));
            continue;
        }
        numbered
            .sort_by(|(an, ae), (bn, be)| an.cmp(bn).then_with(|| ae.filename.cmp(&be.filename)));
        let parts: Vec<AudiobookStatEntry> = numbered.into_iter().map(|(_, e)| e).collect();
        // The group key is the *first part's* real file path, not a
        // virtual dir-level path. Real file paths can't collide with the
        // mp3 parent-dir keys or other singles, and when `Pt2` shows up
        // next to an already-indexed lone `Pt1` the group keeps `Pt1`'s
        // uuid — the diff sees Changed (size grew), not Removed+New, so
        // reading progress survives.
        let group_path = parts[0].filename.clone();
        let total_size_bytes = parts.iter().map(|p| p.size_bytes).sum();
        let max_mtime_epoch = parts.iter().map(|p| p.mtime_epoch).max().unwrap_or(0);
        let uuid = stable_uuid(library_path_key, &group_path);
        groups.push(AudiobookGroup {
            group_path,
            uuid,
            parts,
            total_size_bytes,
            max_mtime_epoch,
            format: ext,
        });
    }
    for (dir, parts) in mp3_buckets {
        let total_size_bytes = parts.iter().map(|p| p.size_bytes).sum();
        let max_mtime_epoch = parts.iter().map(|p| p.mtime_epoch).max().unwrap_or(0);
        let uuid = stable_uuid(library_path_key, &dir);
        groups.push(AudiobookGroup {
            group_path: dir,
            uuid,
            parts,
            total_size_bytes,
            max_mtime_epoch,
            format: "MP3".into(),
        });
    }

    groups.sort_by(|a, b| a.group_path.cmp(&b.group_path));
    groups
}

/// Part-designator keywords accepted before a trailing number. `part`
/// must precede `pt` so the longer keyword wins the suffix match.
const PART_KEYWORDS: &[&str] = &["part", "disc", "disk", "pt", "cd"];

/// Strip an end-anchored part designator from a file stem:
/// `"Dracula Pt1"` → `Some(("Dracula", 1))`, `"Dracula"` → `None`.
///
/// Accepted forms (case-insensitive): keyword + number (`Pt1`, `Pt. 2`,
/// `Part 3`, `Disc 1`, `CD2`), `"<n> of <m>"`, either optionally in
/// trailing parentheses (`(Part 1)`, `(1 of 3)`), and `-`/`_`-delimited
/// bare numbers (`Title - 2`, `Title_2`). A bare space-delimited number
/// (`Foundation 2`) deliberately does NOT match — flat series folders
/// name distinct books that way.
pub(crate) fn strip_part_suffix(stem: &str) -> Option<(String, u32)> {
    // ASCII-lowercasing preserves byte offsets, so indexes computed on
    // `lower` slice `stem` correctly.
    let lower = stem.to_ascii_lowercase();
    let body = lower.trim_end();

    // "Title (Part 1)" / "Title (1 of 3)" — parse the parenthesized tail.
    if let Some(inner_end) = body.strip_suffix(')') {
        if let Some(open) = inner_end.rfind('(') {
            if let Some(n) = parse_part_token(&inner_end[open + 1..]) {
                let base = trim_separators(&stem[..open]);
                if !base.is_empty() {
                    return Some((base.to_string(), n));
                }
            }
        }
    }

    // "Title 1 of 3" — the explicit "of" makes a space delimiter safe.
    let toks: Vec<&str> = body.split_whitespace().collect();
    if toks.len() >= 4 && toks[toks.len() - 2] == "of" {
        let n_tok = toks[toks.len() - 3];
        let m_tok = toks[toks.len() - 1];
        if is_digits(n_tok) && is_digits(m_tok) {
            if let Ok(n) = n_tok.parse() {
                let cut = n_tok.as_ptr() as usize - body.as_ptr() as usize;
                let base = trim_separators(&stem[..cut]);
                if !base.is_empty() {
                    return Some((base.to_string(), n));
                }
            }
        }
    }

    // Trailing digit run, then either a part keyword or a `-`/`_`
    // delimiter before it.
    let digits_start = body.len() - body.bytes().rev().take_while(u8::is_ascii_digit).count();
    if digits_start == body.len() || digits_start == 0 {
        return None;
    }
    let n: u32 = body[digits_start..].parse().ok()?;

    // Skip separators between the number and whatever precedes it.
    let mut cut = digits_start;
    let mut saw_dash = false;
    while cut > 0 {
        let c = body.as_bytes()[cut - 1];
        match c {
            b' ' | b'.' => cut -= 1,
            b'-' | b'_' => {
                saw_dash = true;
                cut -= 1;
            }
            _ => break,
        }
    }

    for kw in PART_KEYWORDS {
        if body[..cut].ends_with(kw) {
            let kw_start = cut - kw.len();
            // The keyword must be its own word: start-of-string or a
            // separator before it ("Sandisk 2" must not match "disk").
            let bounded =
                kw_start == 0 || matches!(body.as_bytes()[kw_start - 1], b' ' | b'-' | b'_' | b'.');
            if bounded {
                let base = trim_separators(&stem[..kw_start]);
                if !base.is_empty() {
                    return Some((base.to_string(), n));
                }
            }
        }
    }

    if saw_dash {
        let base = trim_separators(&stem[..cut]);
        if !base.is_empty() {
            return Some((base.to_string(), n));
        }
    }
    None
}

/// Parse a complete part token: `"part 1"`, `"pt.2"`, `"disc 3"`,
/// `"1 of 3"`. The whole input must be consumed; bare digits are
/// rejected (a trailing `"(2)"` usually marks a duplicate download,
/// not a part).
fn parse_part_token(tok: &str) -> Option<u32> {
    let tok = tok.trim();
    let toks: Vec<&str> = tok.split_whitespace().collect();
    if toks.len() == 3 && toks[1] == "of" && is_digits(toks[0]) && is_digits(toks[2]) {
        return toks[0].parse().ok();
    }
    for kw in PART_KEYWORDS {
        if let Some(rest) = tok.strip_prefix(kw) {
            let rest = rest
                .trim_start_matches(|c: char| c.is_whitespace() || matches!(c, '.' | '-' | '_'));
            if !rest.is_empty() && is_digits(rest) {
                return rest.parse().ok();
            }
        }
    }
    None
}

fn is_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

fn trim_separators(s: &str) -> &str {
    s.trim_end_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.' | ','))
}

fn file_stem_of(filename: &str) -> &str {
    std::path::Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
}

fn single(entry: AudiobookStatEntry, format: &str, library_path_key: &str) -> AudiobookGroup {
    // For single-file groups the canonical key is the file path itself,
    // not its parent dir — two `Author/Book.m4b` and
    // `Author/Bonus.m4b` files in the same directory must produce two
    // groups, not one.
    let group_path = entry.filename.clone();
    // The synthetic-error case carries an empty uuid through; recompute
    // here from the group key so downstream code (`diff_library`) sees a
    // proper stable id when the row is real, and an empty one when it's
    // a placeholder.
    let uuid = if entry.uuid.is_empty() {
        String::new()
    } else {
        stable_uuid(library_path_key, &group_path)
    };
    let size = entry.size_bytes;
    let mtime = entry.mtime_epoch;
    AudiobookGroup {
        group_path,
        uuid,
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
            uuid: crate::helpers::stable_uuid("/lib", name),
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
        // Stable uuid sourced from `(library, group_path)`, not from any
        // individual part — invariant the F2.1 progress endpoints rely on.
        assert_eq!(
            g.uuid,
            crate::helpers::stable_uuid("/lib", "Sanderson/Way of Kings")
        );
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
        // Single-file groups sort before the multi-mp3 group when
        // `Author/Title/all.m4b` < `Author/Title` alphabetically? — let's
        // pin the actual order by content rather than position.
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
            uuid: String::new(),
            mtime_epoch: 0,
            size_bytes: 0,
            error: Some("permission denied".into()),
        };
        let groups = group_into_books(vec![bad.clone()], "/lib");
        assert_eq!(groups.len(), 1);
        assert!(groups[0].uuid.is_empty());
        assert_eq!(groups[0].parts.len(), 1);
        assert_eq!(
            groups[0].parts[0].error.as_deref(),
            Some("permission denied")
        );
    }

    #[test]
    fn strip_part_suffix_accepts_part_designators() {
        let cases = [
            ("Dracula Pt1", "Dracula", 1),
            ("Dracula Pt. 2", "Dracula", 2),
            ("Dracula Part 3", "Dracula", 3),
            ("Dracula part 10", "Dracula", 10),
            ("Dracula Disc 1", "Dracula", 1),
            ("Dracula disk 4", "Dracula", 4),
            ("Dracula CD2", "Dracula", 2),
            ("Dracula 1 of 3", "Dracula", 1),
            ("Dracula (1 of 3)", "Dracula", 1),
            ("Dracula (Part 2)", "Dracula", 2),
            ("Dracula - 2", "Dracula", 2),
            ("Dracula_2", "Dracula", 2),
            ("The Stand - Part 01", "The Stand", 1),
            ("Way of Kings Pt2", "Way of Kings", 2),
        ];
        for (stem, base, n) in cases {
            assert_eq!(
                strip_part_suffix(stem),
                Some((base.to_string(), n)),
                "stem {stem:?}"
            );
        }
    }

    #[test]
    fn strip_part_suffix_rejects_non_part_stems() {
        let cases = [
            "Dracula",
            // Bare space-delimited number: flat series folders name
            // distinct books this way (`Dune 1.m4b`, `Dune 2.m4b`).
            "Foundation 2",
            "Fahrenheit 451",
            // Keyword without a word boundary or without digits.
            "Sandisk 2",
            "Dracula CD",
            "Dracula Part",
            // Parenthesized bare number marks a duplicate, not a part.
            "Dracula (2)",
            // Nothing but a designator — base would be empty.
            "Pt1",
            "Part 2",
            "2",
            "",
        ];
        for stem in cases {
            assert_eq!(strip_part_suffix(stem), None, "stem {stem:?}");
        }
    }

    #[test]
    fn two_part_m4bs_in_one_dir_group_with_first_part_as_key() {
        let groups = group_into_books(
            vec![
                entry("Stoker/Dracula Pt2.m4b", 200, 2000),
                entry("Stoker/Dracula Pt1.m4b", 100, 1000),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        // Group key = first part's real path (sorted by part number), so
        // a previously lone Pt1's uuid is preserved when Pt2 arrives.
        assert_eq!(g.group_path, "Stoker/Dracula Pt1.m4b");
        assert_eq!(
            g.uuid,
            crate::helpers::stable_uuid("/lib", "Stoker/Dracula Pt1.m4b")
        );
        assert_eq!(g.format, "M4B");
        assert_eq!(g.parts.len(), 2);
        assert_eq!(g.parts[0].filename, "Stoker/Dracula Pt1.m4b");
        assert_eq!(g.parts[1].filename, "Stoker/Dracula Pt2.m4b");
        assert_eq!(g.total_size_bytes, 3000);
        assert_eq!(g.max_mtime_epoch, 200);
    }

    #[test]
    fn folder_per_book_part_m4bs_group_naturally() {
        let groups = group_into_books(
            vec![
                entry("Stoker/Dracula/Dracula Pt1.m4b", 100, 1000),
                entry("Stoker/Dracula/Dracula Pt2.m4b", 110, 1100),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_path, "Stoker/Dracula/Dracula Pt1.m4b");
        assert_eq!(groups[0].parts.len(), 2);
    }

    #[test]
    fn lone_part_suffixed_m4b_stays_single_with_unchanged_uuid() {
        let groups = group_into_books(vec![entry("Stoker/Dracula Pt1.m4b", 100, 1000)], "/lib");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].group_path, "Stoker/Dracula Pt1.m4b");
        // Identical uuid to the pre-part-grouping behavior.
        assert_eq!(
            groups[0].uuid,
            crate::helpers::stable_uuid("/lib", "Stoker/Dracula Pt1.m4b")
        );
        assert_eq!(groups[0].parts.len(), 1);
    }

    #[test]
    fn distinct_part_sets_in_one_dir_form_separate_groups() {
        let groups = group_into_books(
            vec![
                entry("Audio/Dracula Pt1.m4b", 100, 1000),
                entry("Audio/Dracula Pt2.m4b", 110, 1100),
                entry("Audio/Dune Part 1.m4b", 120, 1200),
                entry("Audio/Dune Part 2.m4b", 130, 1300),
                entry("Audio/Carmilla.m4b", 140, 1400),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 3);
        let dracula = groups
            .iter()
            .find(|g| g.group_path.contains("Dracula"))
            .unwrap();
        let dune = groups
            .iter()
            .find(|g| g.group_path.contains("Dune"))
            .unwrap();
        let carmilla = groups
            .iter()
            .find(|g| g.group_path.contains("Carmilla"))
            .unwrap();
        assert_eq!(dracula.parts.len(), 2);
        assert_eq!(dune.parts.len(), 2);
        assert_eq!(carmilla.parts.len(), 1);
    }

    #[test]
    fn part_numbers_sort_numerically_not_lexicographically() {
        let groups = group_into_books(
            vec![
                entry("A/Book Pt10.m4b", 100, 1000),
                entry("A/Book Pt2.m4b", 110, 1100),
                entry("A/Book Pt1.m4b", 120, 1200),
            ],
            "/lib",
        );
        assert_eq!(groups.len(), 1);
        let names: Vec<&str> = groups[0]
            .parts
            .iter()
            .map(|p| p.filename.as_str())
            .collect();
        assert_eq!(
            names,
            ["A/Book Pt1.m4b", "A/Book Pt2.m4b", "A/Book Pt10.m4b"]
        );
        assert_eq!(groups[0].group_path, "A/Book Pt1.m4b");
    }

    #[test]
    fn m4b_and_m4a_parts_with_same_stem_do_not_mix() {
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
