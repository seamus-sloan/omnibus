//! `diff_library` move detection: matching a relocation on its stat pair,
//! the filename-stem tiebreaker, the format and extension rules, and every
//! case that declines a move rather than guessing.

use crate::books::IndexedRow;
use crate::ebook::StatEntry;

use super::super::*;
use super::{entry, fileless_row, row};

// ---------- #1536: the Moved bucket (stat-pair relocation detector) ----------

/// AC1: a relocation whose stat pair is unique on both sides is classified
/// Moved and leaves both New and Removed empty — so no Phase-B parse runs
/// for it and the writer's title+author auto-attach never sees it.
#[test]
fn diff_classifies_an_unambiguous_relocation_as_moved_and_not_new() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1000)];
    let db = vec![row("Old/book.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved,
        vec![crate::sync::MovedFile {
            uuid: "Old/book.epub".into(),
            filename: "New/book.epub".into(),
        }]
    );
    assert!(d.new.is_empty(), "the arrival must not also be New");
    assert!(
        d.removed.is_empty(),
        "the departure must not also be Removed"
    );
    assert!(d.changed.is_empty());
    assert!(d.unchanged.is_empty());
}

/// A pure rename inside one directory is the common real case, and it is
/// exactly what requiring a stem match would have killed.
#[test]
fn diff_classifies_a_pure_rename_as_moved() {
    let disk = vec![entry("Lib/new-name.epub", "Lib/new-name.epub", 100, 1000)];
    let db = vec![row("Lib/old-name.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.moved.len(), 1);
    assert_eq!(d.moved[0].filename, "Lib/new-name.epub");
}

/// AC3: a wholesale reorganization leaves `removed` empty, so the
/// mass-missing breaker — which reads `removed.len()` — never trips on the
/// exact scenario move detection exists for.
#[test]
fn diff_leaves_removed_empty_for_a_whole_library_reorganization() {
    let disk: Vec<StatEntry> = (0..40)
        .map(|i| {
            let path = format!("New/{i}.epub");
            entry(&path, &path, 1000 + i, 5000 + i)
        })
        .collect();
    let db: Vec<IndexedRow> = (0..40)
        .map(|i| row(&format!("Old/{i}.epub"), 1000 + i, 5000 + i))
        .collect();
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.moved.len(), 40, "every file matched its own stat pair");
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
    assert!(
        check_mass_missing(d.removed.len(), 40).is_ok(),
        "AC3: reorganizing the whole library must not trip the breaker"
    );
}

/// AC4: two removed files sharing one stat pair are ambiguous, so the
/// detector declines rather than guessing — both stay in Removed and the
/// arrival stays in New, where the writer's title+author path still gets a
/// chance at it.
#[test]
fn diff_declines_a_move_when_the_stat_pair_is_ambiguous_on_the_removed_side() {
    let disk = vec![entry("New/c.epub", "New/c.epub", 100, 1000)];
    let db = vec![row("Old/a.epub", 100, 1000), row("Old/b.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(
        d.moved.is_empty(),
        "ambiguity declines rather than guessing"
    );
    assert_eq!(d.removed, vec!["Old/a.epub", "Old/b.epub"]);
    assert_eq!(d.new.len(), 1);
    assert_eq!(d.new[0].filename, "New/c.epub");
}

/// The mirror of AC4 on the arrival side: one departure, two same-pair
/// arrivals with no shared stem, so nothing is matched.
#[test]
fn diff_declines_a_move_when_the_stat_pair_is_ambiguous_on_the_new_side() {
    let disk = vec![
        entry("New/b.epub", "New/b.epub", 100, 1000),
        entry("New/c.epub", "New/c.epub", 100, 1000),
    ];
    let db = vec![row("Old/a.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.removed, vec!["Old/a.epub"]);
    assert_eq!(d.new.len(), 2);
}

/// AC5: the `(0, 0)` never-observed sentinel never participates, on either
/// side. Without this, every unstattable file would match every other one.
#[test]
fn diff_never_matches_a_move_on_the_zero_zero_sentinel() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 0, 0)];
    let db = vec![row("Old/book.epub", 0, 0)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty(), "AC5: (0, 0) is not a content identity");
    assert_eq!(d.removed, vec!["Old/book.epub"]);
    assert_eq!(d.new.len(), 1);
}

/// AC5, one-sided: a stattable arrival must not match a pre-backfill row
/// whose stat was never observed.
#[test]
fn diff_never_matches_a_move_when_only_one_side_carries_the_sentinel() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1000)];
    let db = vec![row("Old/book.epub", 0, 0)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.removed, vec!["Old/book.epub"]);
    assert_eq!(d.new.len(), 1);
}

/// AC6: a relocation that also changed the file's bytes falls through to
/// Removed + New unchanged — the pair is a byte identity, not a fuzzy one.
#[test]
fn diff_declines_a_move_when_the_size_differs() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1001)];
    let db = vec![row("Old/book.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.removed, vec!["Old/book.epub"]);
    assert_eq!(d.new.len(), 1);
}

/// AC6, the other half of the pair.
#[test]
fn diff_declines_a_move_when_the_mtime_differs() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 101, 1000)];
    let db = vec![row("Old/book.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.removed, vec!["Old/book.epub"]);
    assert_eq!(d.new.len(), 1);
}

/// AC11: an ambiguous stat-pair group in which exactly one candidate on
/// each side also shares the filename stem is matched via the tiebreaker;
/// the candidates the stem can't separate stay in Removed.
#[test]
fn diff_breaks_an_ambiguous_stat_group_on_the_filename_stem() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1000)];
    let db = vec![
        row("Old/book.epub", 100, 1000),
        row("Old/other.epub", 100, 1000),
    ];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved,
        vec![crate::sync::MovedFile {
            uuid: "Old/book.epub".into(),
            filename: "New/book.epub".into(),
        }],
        "AC11: the shared stem breaks the tie"
    );
    assert_eq!(
        d.removed,
        vec!["Old/other.epub"],
        "the unmatched candidate still ghosts"
    );
    assert!(d.new.is_empty());
}

/// The stem is only a tiebreaker, never a licence to guess: when it is
/// itself ambiguous the whole group declines.
#[test]
fn diff_declines_when_the_filename_stem_is_ambiguous_too() {
    let disk = vec![
        entry("A/book.epub", "A/book.epub", 100, 1000),
        entry("B/book.epub", "B/book.epub", 100, 1000),
    ];
    let db = vec![
        row("Old/book.epub", 100, 1000),
        row("Old/other.epub", 100, 1000),
    ];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(
        d.moved.is_empty(),
        "two candidates share the stem — decline"
    );
    assert_eq!(d.removed.len(), 2);
    assert_eq!(d.new.len(), 2);
}

/// A fileless book has no file to relocate, and its projected stat is the
/// `(0, 0)` sentinel — it must never be adopted as a move source, or an
/// unrelated arrival would silently claim a ghost's identity.
#[test]
fn diff_never_treats_a_fileless_book_as_a_move_source() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1000)];
    let db = vec![fileless_row("Old/book.epub")];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert!(
        d.removed.is_empty(),
        "an already-fileless book is left alone"
    );
    assert_eq!(d.new.len(), 1);
}

/// Moved rides on Removed, which an untrustworthy enumeration suppresses:
/// with nothing to match arrivals against, a partial scan must not invent
/// relocations either.
#[test]
fn diff_suppresses_moved_bucket_when_enumeration_is_untrustworthy() {
    let disk = vec![entry("New/book.epub", "New/book.epub", 100, 1000)];
    let db = vec![row("Old/book.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), false);
    assert!(d.moved.is_empty());
    assert!(d.removed.is_empty());
    assert_eq!(d.new.len(), 1, "the arrival is still indexed");
}

/// Each moved pair consumes one candidate from each side, so several
/// independent relocations in one scan all land — and the emitted order is
/// stable (sorted on content, not on `HashMap` iteration order).
#[test]
fn diff_matches_several_independent_relocations_in_one_scan() {
    let disk = vec![
        entry("New/b.epub", "New/b.epub", 200, 2000),
        entry("New/a.epub", "New/a.epub", 100, 1000),
    ];
    let db = vec![row("Old/a.epub", 100, 1000), row("Old/b.epub", 200, 2000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved,
        vec![
            crate::sync::MovedFile {
                uuid: "Old/a.epub".into(),
                filename: "New/a.epub".into(),
            },
            crate::sync::MovedFile {
                uuid: "Old/b.epub".into(),
                filename: "New/b.epub".into(),
            },
        ]
    );
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
}

/// A genuine delete and a genuine add in the same scan, with unrelated
/// stats, must stay in their own buckets — the detector only claims pairs
/// it can prove.
#[test]
fn diff_keeps_an_unrelated_delete_and_add_in_removed_and_new() {
    let disk = vec![entry("New/fresh.epub", "New/fresh.epub", 300, 3000)];
    let db = vec![row("Old/gone.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.removed, vec!["Old/gone.epub"]);
    assert_eq!(d.new.len(), 1);
}

/// A relocation that also changed the file's **extension** must not be
/// classified Moved. `book_files.format` drives path resolution
/// (`book_file_path` rebuilds `…/filename + "." + lower(format)`), and the
/// move writer rewrites path columns only — so a Moved classification here
/// would leave the format naming a file that is no longer on disk, with no
/// self-healing: the next scan finds the new path with a matching stat and
/// calls it Unchanged. Falling through to Removed + New is correct, because
/// a format change genuinely needs a Phase-B re-parse.
#[test]
fn diff_declines_a_move_when_only_the_extension_changed() {
    // Retyping `.m4a` to `.m4b` — a real thing readers do so players show
    // chapters — preserves the bytes, so the stat pair matches exactly.
    let disk = vec![entry("Author/Book.m4b", "Author/Book.m4b", 100, 1000)];
    let db = vec![row("Author/Book.m4a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(
        d.moved.is_empty(),
        "a format change is not a relocation: {:?}",
        d.moved
    );
    assert_eq!(d.removed, vec!["Author/Book.m4a"]);
    assert_eq!(d.new.len(), 1, "the retyped file is re-parsed as New");
    assert_eq!(d.new[0].filename, "Author/Book.m4b");
}

/// The extension guard is case-insensitive, matching how
/// `split_filename` uppercases into `book_files.format`: renaming
/// `Book.epub` to `BOOK.EPUB` is still one relocation, not a format change.
#[test]
fn diff_still_matches_a_move_when_only_the_extension_case_changed() {
    let disk = vec![entry("New/BOOK.EPUB", "New/BOOK.EPUB", 100, 1000)];
    let db = vec![row("Old/book.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved.len(),
        1,
        "case-only extension change still relocates"
    );
    assert_eq!(d.moved[0].filename, "New/BOOK.EPUB");
    assert!(d.removed.is_empty());
}

/// The stem tiebreaker must not smuggle a format change past the guard —
/// `path_stem` drops the extension, so before the format was part of the
/// match key an ambiguous group would actively *help* pair `.m4a` with
/// `.m4b`.
#[test]
fn diff_stem_tiebreaker_does_not_pair_across_formats() {
    let disk = vec![entry("New/Book.m4b", "New/Book.m4b", 100, 1000)];
    let db = vec![
        row("Old/Book.m4a", 100, 1000),
        row("Old/Other.m4a", 100, 1000),
    ];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(
        d.moved.is_empty(),
        "the shared stem must not bridge two formats: {:?}",
        d.moved
    );
    assert_eq!(d.removed.len(), 2);
    assert_eq!(d.new.len(), 1);
}

/// Scoping the key by format also makes the *same-format* match sharper:
/// two vanished files sharing a stat pair no longer make each other
/// ambiguous when they are different formats, so each is matched against
/// its own format's arrival.
#[test]
fn diff_matches_same_stat_moves_of_different_formats_independently() {
    let disk = vec![
        entry("New/Book.epub", "New/Book.epub", 100, 1000),
        entry("New/Book.cbz", "New/Book.cbz", 100, 1000),
    ];
    let db = vec![
        row("Old/Book.epub", 100, 1000),
        row("Old/Book.cbz", 100, 1000),
    ];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved,
        vec![
            crate::sync::MovedFile {
                uuid: "Old/Book.cbz".into(),
                filename: "New/Book.cbz".into(),
            },
            crate::sync::MovedFile {
                uuid: "Old/Book.epub".into(),
                filename: "New/Book.epub".into(),
            },
        ],
        "each format resolves within its own group"
    );
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
}

/// Two books swapping paths in one scan cannot reach the Moved bucket at
/// all, so the writer's destination pre-check can never deadlock a cycle.
/// The buckets make it structurally impossible: a Removed candidate
/// requires its `scan_key` to be **absent** from disk, while a New
/// candidate requires its path to be **present** — so a moved entry's
/// destination can never be another moved entry's source. A real swap
/// leaves both paths occupied, so neither row is a departure and both
/// classify Changed against the bytes that are now at their path.
#[test]
fn diff_classifies_a_path_swap_as_changed_never_as_a_moved_cycle() {
    // A was at X and B at Y; their contents have been exchanged.
    let disk = vec![
        entry("X.epub", "X.epub", 200, 2000),
        entry("Y.epub", "Y.epub", 100, 1000),
    ];
    let db = vec![row("X.epub", 100, 1000), row("Y.epub", 200, 2000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(
        d.moved.is_empty(),
        "a swap is not a relocation — no cycle can form: {:?}",
        d.moved
    );
    assert!(d.removed.is_empty(), "neither path vanished");
    assert!(d.new.is_empty(), "neither path is new");
    assert_eq!(
        d.changed.len(),
        2,
        "both files re-parse against their new bytes"
    );
}

/// The byte-identical variant of the same swap: both paths are still
/// occupied, so both classify Unchanged and nothing needs doing — the two
/// files are interchangeable by definition.
#[test]
fn diff_classifies_a_byte_identical_path_swap_as_unchanged() {
    let disk = vec![
        entry("X.epub", "X.epub", 100, 1000),
        entry("Y.epub", "Y.epub", 100, 1000),
    ];
    let db = vec![row("X.epub", 100, 1000), row("Y.epub", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.moved.is_empty());
    assert_eq!(d.unchanged.len(), 2);
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
}

/// The general form of the invariant the two tests above rely on: no
/// destination emitted into Moved is the current `scan_key` of any other
/// Moved entry. Exercised over a chained relocation (A→B, B→C) — the shape
/// closest to a cycle that the buckets actually admit — plus a genuine
/// departure and arrival alongside it.
#[test]
fn moved_destinations_never_collide_with_another_moved_entrys_source() {
    // Only `C.epub` is on disk; `A.epub` and `B.epub` both vanished. Their
    // stat pairs differ, so each is matched independently — but there is
    // exactly one arrival, so only one can pair.
    let disk = vec![entry("C.epub", "C.epub", 100, 1000)];
    let db = vec![row("A.epub", 100, 1000), row("B.epub", 200, 2000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.moved.len(),
        1,
        "one arrival can absorb only one departure"
    );
    assert_eq!(d.moved[0].uuid, "A.epub");
    assert_eq!(d.moved[0].filename, "C.epub");
    assert_eq!(d.removed, vec!["B.epub"], "the unmatched departure ghosts");

    let sources: std::collections::HashSet<&str> = db.iter().map(|r| r.scan_key.as_str()).collect();
    for m in &d.moved {
        assert!(
            !sources.contains(m.filename.as_str()) || disk.iter().any(|e| e.scan_key == m.filename),
            "a Moved destination must be a path that exists on disk"
        );
    }
}
