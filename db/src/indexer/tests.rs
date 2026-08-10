//! Unit tests for the `indexer` module — `diff_library` classifiers,
//! `is_stale` window logic, `reindex` preservation-on-failure, the
//! incomplete-enumeration data-loss guard, and the shared-path
//! cross-format deletion guard.

use super::*;
use crate::books::{
    list_books, list_indexed_rows_for_formats, list_merged_rows_for_formats, IndexedRow,
};
use crate::ebook::StatEntry;
use crate::pool::init_db;
use crate::sync::{replace_books, sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{
    build_stored_zip, count_rows, indexed, indexed_audiobook, indexed_with_stat, make_test_dir,
    uuid_by_scan_key, CoversTempDir, EnvVarGuard,
};

/// Seed a `scan_roots` row for `path` with an explicit `last_indexed`
/// epoch-seconds value. There's no public writer for `last_indexed`
/// that lets a test set an arbitrary timestamp (`sync_books` always
/// stamps "now"), so the `is_stale` window tests insert the row
/// directly — exactly the columns `last_indexed_at` reads back.
async fn seed_last_indexed(pool: &SqlitePool, path: &str, last_indexed: i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name, last_indexed) VALUES (?, ?, ?)")
        .bind(path)
        .bind(path)
        .bind(last_indexed)
        .execute(pool)
        .await
        .unwrap();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64
}

fn entry(name: &str, scan_key: &str, mtime: i64, size: i64) -> StatEntry {
    StatEntry {
        filename: name.into(),
        scan_key: scan_key.into(),
        mtime_epoch: mtime,
        size_bytes: size,
        error: None,
    }
}
/// A file-backed DB row. `scan_key` is the diff key; `uuid` mirrors it so
/// the Removed/Backfill buckets (which carry uuids) keep asserting on the
/// same string the callers pass.
fn row(scan_key: &str, mtime: i64, size: i64) -> IndexedRow {
    IndexedRow {
        uuid: scan_key.into(),
        scan_key: scan_key.into(),
        has_file: true,
        mtime_epoch: mtime,
        size_bytes: size,
    }
}
/// A fileless book row: retained book whose file is gone.
fn fileless_row(scan_key: &str) -> IndexedRow {
    IndexedRow {
        uuid: scan_key.into(),
        scan_key: scan_key.into(),
        has_file: false,
        mtime_epoch: 0,
        size_bytes: 0,
    }
}

#[test]
fn diff_classifies_new_file_as_new() {
    let disk = vec![entry("a.epub", "uuid-a", 100, 1000)];
    let db: Vec<IndexedRow> = vec![];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.new.len(), 1);
    assert_eq!(d.new[0].filename, "a.epub");
    assert_eq!(d.new[0].mtime_epoch, 100);
    assert_eq!(d.new[0].size_bytes, 1000);
    assert!(d.changed.is_empty());
    assert!(d.unchanged.is_empty());
    assert!(d.removed.is_empty());
    assert!(d.backfill.is_empty());
}

#[test]
fn diff_classifies_missing_file_as_removed() {
    let disk: Vec<StatEntry> = vec![];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.removed, vec!["uuid-a".to_string()]);
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
    assert!(d.unchanged.is_empty());
}

#[test]
fn diff_classifies_matching_stat_as_unchanged() {
    let disk = vec![entry("a.epub", "uuid-a", 100, 1000)];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.unchanged, vec!["uuid-a".to_string()]);
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
    assert!(d.removed.is_empty());
    assert!(d.backfill.is_empty());
}

#[test]
fn diff_classifies_mtime_drift_as_changed() {
    let disk = vec![entry("a.epub", "uuid-a", 200, 1000)];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].mtime_epoch, 200);
    assert!(d.unchanged.is_empty());
}

#[test]
fn diff_classifies_size_drift_as_changed() {
    let disk = vec![entry("a.epub", "uuid-a", 100, 2000)];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].size_bytes, 2000);
}

#[test]
fn diff_routes_zero_zero_sentinel_to_backfill_not_changed() {
    // Migration default: existing rows look like (0, 0). The disk
    // has real values — that combination must NOT trigger a full
    // re-parse on the first post-migration reindex.
    let disk = vec![entry("a.epub", "uuid-a", 100, 1000)];
    let db = vec![row("uuid-a", 0, 0)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.backfill, vec![("uuid-a".into(), 100, 1000)]);
    assert!(d.changed.is_empty());
    assert!(d.new.is_empty());
    assert!(d.unchanged.is_empty());
}

#[test]
fn diff_handles_mixed_buckets_in_one_call() {
    let disk = vec![
        entry("keep.epub", "uuid-keep", 100, 1000),
        entry("edit.epub", "uuid-edit", 250, 1100),
        entry("add.epub", "uuid-add", 300, 500),
        entry("backfill.epub", "uuid-bf", 400, 600),
    ];
    let db = vec![
        row("uuid-keep", 100, 1000),
        row("uuid-edit", 200, 1000),
        row("uuid-bf", 0, 0),
        row("uuid-gone", 50, 200),
    ];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.unchanged, vec!["uuid-keep".to_string()]);
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].filename, "edit.epub");
    assert_eq!(d.new.len(), 1);
    assert_eq!(d.new[0].filename, "add.epub");
    assert_eq!(d.removed, vec!["uuid-gone".to_string()]);
    assert_eq!(d.backfill, vec![("uuid-bf".into(), 400, 600)]);
}

#[test]
fn diff_ignores_empty_uuid_placeholders_from_stat_walk() {
    // `stat_ebook_library` emits a synthetic empty-uuid entry for
    // unreadable subdirs. The diff must not treat those as books.
    let disk = vec![
        entry("good.epub", "uuid-good", 100, 1000),
        StatEntry {
            filename: "bad/".into(),
            scan_key: String::new(),
            mtime_epoch: 0,
            size_bytes: 0,
            error: Some("permission denied".into()),
        },
    ];
    let db: Vec<IndexedRow> = vec![];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(d.new.len(), 1);
    assert_eq!(d.new[0].filename, "good.epub");
}

#[test]
fn diff_routes_returning_file_for_a_fileless_book_through_changed() {
    // A fileless book (F2) whose file is back on disk must classify Changed
    // — so the writer re-attaches, preserving the existing uuid — not New
    // (which would mint a fresh uuid and orphan its soft-ref user data).
    let disk = vec![entry("a.epub", "a.epub", 100, 1000)];
    let db = vec![fileless_row("a.epub")];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.new.is_empty(), "a fileless match is not New");
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].filename, "a.epub");
    assert!(d.removed.is_empty());
}

#[test]
fn diff_leaves_a_still_missing_fileless_book_untouched() {
    // A fileless book whose file is still gone stays fileless — not re-Removed.
    let disk: Vec<StatEntry> = vec![];
    let db = vec![fileless_row("a.epub")];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
}

#[test]
fn diff_builds_absolute_paths_for_parse_targets() {
    let disk = vec![entry("sub/a.epub", "uuid-a", 100, 1000)];
    let d = diff_library(&disk, &[], Path::new("/srv/library"), true);
    assert_eq!(d.new[0].absolute, Path::new("/srv/library/sub/a.epub"));
}

#[test]
fn diff_suppresses_removed_bucket_when_enumeration_is_untrustworthy() {
    // #819: on a partial/empty enumeration the caller passes
    // `enumeration_trustworthy = false`, and the Removed bucket must stay
    // empty — nothing is flagged missing even though the disk set is empty.
    let disk: Vec<StatEntry> = vec![];
    let db = vec![row("uuid-a", 100, 1000), row("uuid-b", 200, 2000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), false);
    assert!(
        d.removed.is_empty(),
        "an untrusted enumeration must not remove anything, got {:?}",
        d.removed
    );
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
}

#[test]
fn diff_still_indexes_new_files_when_enumeration_is_untrustworthy() {
    // Distrust gates only the Removed bucket — a partial scan still safely
    // indexes the files it did see (New/Changed/Backfill unaffected).
    let disk = vec![entry("new.epub", "uuid-new", 100, 1000)];
    let db = vec![row("uuid-old", 50, 500)];
    let d = diff_library(&disk, &db, Path::new("/lib"), false);
    assert_eq!(d.new.len(), 1, "new files still index on an untrusted scan");
    assert_eq!(d.new[0].filename, "new.epub");
    assert!(
        d.removed.is_empty(),
        "the un-enumerated old book is not removed"
    );
}

#[test]
fn diff_isolates_an_unreadable_file_from_its_siblings_in_the_same_root() {
    // Acceptance (b): a single file the walker couldn't stat is still
    // enumerated (read_dir listed it) — its `stat_file` degraded to (0, 0),
    // so the diff re-parses it in place via Changed (harmless, preserves the
    // id). Critically, it must NOT knock any sibling in the same root into
    // Removed; the readable sibling stays Unchanged.
    let disk = vec![
        entry("good.epub", "uuid-good", 100, 1000),
        entry("bad.epub", "uuid-bad", 0, 0),
    ];
    let db = vec![row("uuid-good", 100, 1000), row("uuid-bad", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"), true);
    assert_eq!(
        d.unchanged,
        vec!["uuid-good".to_string()],
        "the readable sibling stays Unchanged"
    );
    assert_eq!(
        d.changed.len(),
        1,
        "the unreadable file re-parses in place, not removed"
    );
    assert_eq!(d.changed[0].filename, "bad.epub");
    assert!(
        d.removed.is_empty(),
        "an unreadable file must not remove any sibling, got {:?}",
        d.removed
    );
}

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

#[test]
fn enumeration_trustworthy_true_for_healthy_populated_scan() {
    // Complete walk, files present (saw_any_file), DB has files — normal case.
    assert!(enumeration_is_trustworthy(false, true, true));
}

#[test]
fn enumeration_untrustworthy_when_incomplete() {
    // A subdir read failed — partial view, distrust regardless of the rest.
    assert!(!enumeration_is_trustworthy(true, true, true));
}

#[test]
fn enumeration_untrustworthy_when_populated_root_reads_totally_empty() {
    // The boot-race / unmounted-NFS case: the walk saw NO file of any
    // extension but the DB still holds file-backed books. Distrust so the
    // removal pass is skipped.
    assert!(!enumeration_is_trustworthy(false, false, true));
}

#[test]
fn enumeration_trustworthy_when_root_has_files_of_another_format() {
    // #328 shared-path: the root has files (saw_any_file), just none of this
    // library's format. That is a legitimate empty diff, not a fault — trust
    // it so the cross-format removal still works.
    assert!(enumeration_is_trustworthy(false, true, true));
}

#[test]
fn enumeration_trustworthy_when_empty_root_matches_empty_db() {
    // A genuinely empty library (or first-ever scan) must stay indexable —
    // an empty read is only suspicious when the DB disagrees.
    assert!(enumeration_is_trustworthy(false, false, false));
}

#[test]
fn check_mass_missing_allows_removals_within_the_threshold() {
    // 20 of 100 (20%) is at the boundary, not over it — allowed.
    assert!(check_mass_missing(20, 100).is_ok());
    // 21 of 100 (21%) trips the breaker.
    assert!(check_mass_missing(21, 100).is_err());
}

#[test]
fn check_mass_missing_allows_small_absolute_removals_regardless_of_percent() {
    // Deleting the only book in a 1-book library is 100% but under the
    // absolute floor, so it must not trip the breaker.
    assert!(check_mass_missing(1, 1).is_ok());
    assert!(check_mass_missing(MASS_MISSING_MIN_ABSOLUTE, MASS_MISSING_MIN_ABSOLUTE).is_ok());
}

#[test]
fn check_mass_missing_reports_counts_and_percent_in_the_error() {
    let err = check_mass_missing(50, 100).unwrap_err();
    assert_eq!(err.removed, 50);
    assert_eq!(err.total, 100);
    assert!((err.percent - 50.0).abs() < f64::EPSILON);
}

// ---------- #1057: warn threshold (the sub-abort middle ground) ----------

#[test]
fn ghost_warning_threshold_exceeded_is_false_below_the_warn_fraction() {
    // 10 of 100 (10%) sits at the warn boundary, not over it — silent.
    assert!(!ghost_warning_threshold_exceeded(10, 100));
    // 9 of 100 (9%) is comfortably under the warn fraction.
    assert!(!ghost_warning_threshold_exceeded(9, 100));
}

#[test]
fn ghost_warning_threshold_exceeded_is_false_under_the_absolute_floor_regardless_of_percent() {
    // Deleting the only book in a 1-book library is 100% but under the
    // absolute floor shared with the abort guard — never warns.
    assert!(!ghost_warning_threshold_exceeded(1, 1));
    assert!(!ghost_warning_threshold_exceeded(
        MASS_MISSING_MIN_ABSOLUTE,
        MASS_MISSING_MIN_ABSOLUTE
    ));
}

#[test]
fn ghost_warning_threshold_exceeded_is_false_when_the_library_has_no_file_backed_books() {
    assert!(!ghost_warning_threshold_exceeded(50, 0));
}

#[test]
fn ghost_warning_threshold_exceeded_is_true_in_the_warn_band_below_the_abort_guard() {
    // 15 of 100 (15%) clears the 10% warn fraction but stays under the 20%
    // abort fraction — the sub-abort middle ground this issue adds.
    assert!(ghost_warning_threshold_exceeded(15, 100));
    assert!(
        check_mass_missing(15, 100).is_ok(),
        "the warn band must not trip the #819 abort guard"
    );
}

#[test]
fn ghost_warning_threshold_exceeded_is_true_up_to_the_abort_boundary() {
    // 20 of 100 (20%) is the abort guard's own boundary (still allowed
    // through by check_mass_missing) and clears the warn fraction too.
    assert!(ghost_warning_threshold_exceeded(20, 100));
    assert!(check_mass_missing(20, 100).is_ok());
    // 21 of 100 (21%) crosses into the abort guard.
    assert!(check_mass_missing(21, 100).is_err());
}

#[test]
fn reindex_stats_ghost_warning_is_none_below_the_warn_threshold() {
    let stats = ReindexStats {
        removed: 5,
        file_backed_total: 100,
        ..Default::default()
    };
    assert_eq!(stats.ghost_warning(), None);
}

#[test]
fn reindex_stats_ghost_warning_carries_the_removed_and_total_counts_in_the_warn_band() {
    let stats = ReindexStats {
        removed: 15,
        file_backed_total: 100,
        ..Default::default()
    };
    assert_eq!(
        stats.ghost_warning(),
        Some(omnibus_shared::GhostFilesWarning {
            removed: 15,
            total: 100,
        })
    );
}

#[test]
fn is_stale_decision_respects_window_boundaries() {
    // Pure window logic: not stale strictly inside the window, stale at
    // and past the horizon.
    let last = 1_700_000_000;
    assert!(!is_stale_decision(last, last));
    assert!(!is_stale_decision(last, last + REFRESH_AFTER_SECS - 1));
    assert!(is_stale_decision(last, last + REFRESH_AFTER_SECS));
    assert!(is_stale_decision(last, last + REFRESH_AFTER_SECS + 1));
}

#[test]
fn is_stale_decision_clock_failure_serves_stale() {
    // The clock-failure fallback in `is_stale` substitutes `last` for an
    // unreadable `now`, so the decision is evaluated with `now == last`.
    // Pin the documented consequence: not stale (serve the existing index
    // rather than thrash the disk on every poll).
    let last = 1_700_000_000;
    assert!(!is_stale_decision(last, last));
}

#[tokio::test]
async fn is_stale_returns_true_when_no_index_exists() {
    // Fresh DB: the library has never been indexed, so `last_indexed_at`
    // is None and `is_stale` short-circuits to true (kick off the first
    // index). No `libraries` row at all is the strongest form of this.
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn is_stale_returns_false_within_window() {
    // Indexed 30s ago — well inside REFRESH_AFTER_SECS (1h), so no
    // reindex is due yet.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_last_indexed(&pool, "/lib", now_secs() - 30).await;
    assert!(!is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn is_stale_returns_true_past_window() {
    // Indexed just past the refresh horizon (REFRESH_AFTER_SECS + 1
    // seconds ago) — the index is stale and a reindex is due.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_last_indexed(&pool, "/lib", now_secs() - REFRESH_AFTER_SECS - 1).await;
    assert!(is_stale(&pool, "/lib").await.unwrap());
}

#[tokio::test]
async fn reindex_propagates_scan_error_without_clearing_existing_index() {
    // Preserve-on-failure invariant: a fatal scan error (here, a
    // library path that doesn't exist on disk) must return Err and
    // leave the existing index completely untouched — we'd rather
    // serve stale-but-good data than wipe the table. Seed one book,
    // point `reindex` at a non-existent directory, and assert both the
    // Err and that the original row survives.
    let _covers = CoversTempDir::new("reindex-preserve");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // A path under a temp dir that we never create — `stat_ebook_library`
    // reports `path not found`, which `reindex` turns into a bail!.
    let missing_path = std::env::temp_dir().join(format!("omnibus-nonexistent-{}", now_secs()));
    assert!(
        !missing_path.exists(),
        "test precondition: the library path must not exist"
    );
    let missing = missing_path.to_string_lossy().into_owned();

    replace_books(
        &pool,
        &missing,
        vec![indexed(
            "a.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    assert_eq!(
        list_books(&pool, &missing).await.unwrap().len(),
        1,
        "test precondition: the book row must be seeded"
    );

    let result = reindex(&pool, &missing).await;
    assert!(
        result.is_err(),
        "reindex must surface the fatal scan error as Err"
    );

    // The pre-existing index is intact — reindex never touched the DB.
    let after = list_books(&pool, &missing).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "a failed scan must preserve the existing index"
    );
    assert_eq!(after[0].title.as_deref(), Some("Dracula"));
}

// ---------- #328: shared-path cross-format deletion guard ----------

/// Seed one `IndexedAudiobook` row into the DB at `library_path`.
/// Bypasses the full audiobook indexer (no real .m4b file needed) so
/// the cross-format-deletion regression tests can focus on the
/// reindex read-side filter, not on parsing real audio files.
async fn seed_audiobook_row(pool: &SqlitePool, library_path: &str, group_path: &str) {
    let plan = crate::sync::AudiobookSyncPlan {
        new_books: vec![crate::audiobook::IndexedAudiobook {
            scan_key: group_path.to_string(),
            group_path: group_path.to_string(),
            format: "M4B".to_string(),
            title: "AudioTitle".to_string(),
            creator_name: None,
            cover: None,
            accent: None,
            parts: vec![],
            chapters: vec![],
            total_size_bytes: 100,
            max_mtime_epoch: 100,
            description: None,
            error: None,
        }],
        ..Default::default()
    };
    crate::sync::sync_audiobooks(pool, library_path, plan)
        .await
        .unwrap();
}

#[tokio::test]
async fn reindex_audiobooks_does_not_delete_ebook_rows_when_libraries_share_a_path() {
    // #328: when ebook_library_path == audiobook_library_path, an
    // audiobook reindex must not classify every EPUB row as Removed
    // and delete it. The fix scopes the diff's "currently indexed"
    // view to the audiobook formats only.
    let _covers = CoversTempDir::new("reindex-audio-shared");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // A real shared dir holding a real EPUB but no audio files — the exact
    // configuration the bug reproduces on. The EPUB file keeps the walk
    // non-empty so the #819 guard trusts the (audio-empty) enumeration and
    // still runs the removal pass for the absent M4B.
    let shared = crate::test_support::make_test_dir("reindex-audio-shared-lib");
    let shared_path = shared.to_string_lossy().into_owned();
    std::fs::write(shared.join("dracula.epub"), b"not a zip").unwrap();

    // Pre-seed one EPUB row and one audiobook row at the same
    // library_path — the exact configuration the bug reproduces on.
    replace_books(
        &pool,
        &shared_path,
        vec![indexed(
            "dracula.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    seed_audiobook_row(&pool, &shared_path, "audio/AudioTitle.m4b").await;
    assert_eq!(
        list_books(&pool, &shared_path).await.unwrap().len(),
        2,
        "test precondition: two books (EPUB + M4B) seeded at the shared path"
    );

    // Run the audiobook reindex. No .m4b files are on disk, but the EPUB
    // keeps the enumeration non-empty (trustworthy), so the audiobook row is
    // classified Removed; the EPUB row survives because its format is
    // outside the audiobook allow-list.
    reindex_audiobooks(&pool, &shared_path).await.unwrap();

    let after = list_books(&pool, &shared_path).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "EPUB row must survive an audiobook-only reindex"
    );
    assert_eq!(after[0].title.as_deref(), Some("Dracula"));
    assert!(
        after[0].formats.iter().any(|f| f == "EPUB"),
        "the surviving row must be the EPUB, not a stray audiobook"
    );

    let _ = std::fs::remove_dir_all(&shared);
}

#[tokio::test]
async fn reindex_ebooks_does_not_delete_audiobook_rows_when_libraries_share_a_path() {
    // #328 inverse: an ebook reindex against a shared path must not
    // delete audiobook rows for the same symmetric reason.
    let _covers = CoversTempDir::new("reindex-ebook-shared");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let shared = crate::test_support::make_test_dir("reindex-ebook-shared-lib");
    let shared_path = shared.to_string_lossy().into_owned();
    // A real (non-epub) audio file keeps the ebook walk non-empty so the
    // #819 guard trusts the (epub-empty) enumeration and still removes the
    // absent EPUB row.
    std::fs::write(shared.join("AudioTitle.m4b"), b"not audio").unwrap();

    replace_books(
        &pool,
        &shared_path,
        vec![indexed(
            "dracula.epub",
            Some("Dracula"),
            &["Stoker"],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();
    seed_audiobook_row(&pool, &shared_path, "audio/AudioTitle.m4b").await;
    assert_eq!(
        list_books(&pool, &shared_path).await.unwrap().len(),
        2,
        "test precondition: two books (EPUB + M4B) seeded at the shared path"
    );

    // Run the ebook reindex. No .epub files are on disk, but the .m4b keeps
    // the enumeration non-empty (trustworthy), so the EPUB row is classified
    // Removed; the M4B row survives because its format is outside the ebook
    // allow-list.
    reindex(&pool, &shared_path).await.unwrap();

    let after = list_books(&pool, &shared_path).await.unwrap();
    assert_eq!(
        after.len(),
        1,
        "audiobook row must survive an ebook-only reindex"
    );
    assert_eq!(after[0].title.as_deref(), Some("AudioTitle"));
    assert!(
        after[0].formats.iter().any(|f| f == "M4B"),
        "the surviving row must be the audiobook, not a stray ebook"
    );

    let _ = std::fs::remove_dir_all(&shared);
}

async fn seed_audiobook_for_backfill(
    pool: &SqlitePool,
    library_path: &str,
    uuid: &str,
    first_part_filename: &str,
    format: &str,
) -> i64 {
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) \
         ON CONFLICT(path) DO NOTHING",
    )
    .bind(library_path)
    .bind(library_path)
    .execute(pool)
    .await
    .unwrap();

    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(library_path)
        .fetch_one(pool)
        .await
        .unwrap();

    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(library_path)
    .bind(uuid)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();

    let book_file_id: i64 = sqlx::query_scalar(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, ?, ?, 100, 100) RETURNING id",
    )
    .bind(book_id)
    .bind(format)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO book_file_parts \
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, ?, 100, 100, 3600.0)",
    )
    .bind(book_file_id)
    .bind(first_part_filename)
    .execute(pool)
    .await
    .unwrap();

    book_file_id
}

#[tokio::test]
async fn backfill_chapters_inserts_synthetic_chapters_for_all_books_in_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = "/tmp/backfill_test_lib";

    let bfid_a = seed_audiobook_for_backfill(&pool, lib, "book-a", "book-a/part.m4b", "M4B").await;
    let bfid_b = seed_audiobook_for_backfill(&pool, lib, "book-b", "book-b/part.m4b", "M4B").await;

    let mut progress_calls: Vec<(u32, u32)> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    backfill_chapters(&pool, lib, |processed, total, item| {
        progress_calls.push((processed, total));
        items.push(item.to_string());
    })
    .await
    .unwrap();

    let chapters_a: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters WHERE book_file_id = ?")
            .bind(bfid_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    let chapters_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters WHERE book_file_id = ?")
            .bind(bfid_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        chapters_a >= 1,
        "book-a must have at least one synthetic chapter after backfill"
    );
    assert!(
        chapters_b >= 1,
        "book-b must have at least one synthetic chapter after backfill"
    );

    assert_eq!(
        progress_calls.len(),
        2,
        "on_progress must be called once per book"
    );
    assert_eq!(progress_calls[0], (1, 2));
    assert_eq!(progress_calls[1], (2, 2));
    // Item paths are the library directory name plus the part's relative
    // path (#1802) — never the absolute `/tmp/...` root.
    assert_eq!(
        items,
        vec![
            "backfill_test_lib/book-a/part.m4b".to_string(),
            "backfill_test_lib/book-b/part.m4b".to_string(),
        ]
    );
}

#[tokio::test]
async fn backfill_chapters_is_idempotent_after_all_books_have_chapters() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = "/tmp/backfill_idempotent_lib";

    let bfid = seed_audiobook_for_backfill(&pool, lib, "book-c", "book-c/part.m4b", "M4B").await;

    sqlx::query(
        "INSERT INTO file_chapters \
            (book_file_id, ordinal, title, start_seconds, duration_seconds) \
         VALUES (?, 0, 'Chapter 1', 0.0, 3600.0)",
    )
    .bind(bfid)
    .execute(&pool)
    .await
    .unwrap();

    let mut progress_calls = 0u32;
    backfill_chapters(&pool, lib, |_, _, _| {
        progress_calls += 1;
    })
    .await
    .unwrap();

    assert_eq!(
        progress_calls, 0,
        "on_progress must not be called when all books already have chapters"
    );
}

// ---------- word-count backfill (migration 0049) ----------

/// Seed an EPUB book backed by a real fixture file on disk, with a NULL
/// `word_count` (the pre-0049 state), so `backfill_word_counts` has a live
/// file to open and a candidate row to fill. `dir` is the `scan_roots.path`
/// the backfill is scoped to; the file lands at `dir/<fixture>`.
async fn seed_ebook_missing_word_count(
    pool: &SqlitePool,
    dir: &std::path::Path,
    fixture_name: &str,
    uuid: &str,
) -> i64 {
    crate::ebook::test_support::copy_fixture_into(fixture_name, dir);
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(dir.to_str().unwrap())
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let stem = fixture_name.trim_end_matches(".epub");
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(pool)
    .await
    .unwrap();
    book_id
}

async fn word_count_of(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn backfill_word_counts_fills_null_rows_from_the_epub_spine() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("wc-backfill");
    // alpha.epub is a 4-word spine, beta.epub a 10-word spine (see
    // `ebook::wordcount::tests`).
    let a = seed_ebook_missing_word_count(&pool, &dir, "alpha.epub", "uuid-a").await;
    let b = seed_ebook_missing_word_count(&pool, &dir, "beta.epub", "uuid-b").await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_word_counts(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(word_count_of(&pool, a).await, Some(4));
    assert_eq!(word_count_of(&pool, b).await, Some(10));
    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_word_counts_is_idempotent_once_filled() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("wc-backfill-idempotent");
    seed_ebook_missing_word_count(&pool, &dir, "alpha.epub", "uuid-a").await;

    backfill_word_counts(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();

    // Second pass: every book now has a count, so there are no candidates
    // and `on_progress` never fires.
    let mut calls = 0u32;
    backfill_word_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- page-count backfill (migration 0063) ----------

/// Seed a CBZ book backed by a real (stored, uncompressed) archive on disk,
/// with a NULL `page_count` (the pre-0063 state), so `backfill_page_counts`
/// has a live file to open and a candidate row to fill.
async fn seed_cbz_missing_page_count(
    pool: &SqlitePool,
    dir: &std::path::Path,
    uuid: &str,
    pages: usize,
) -> i64 {
    let lib_str = dir.to_str().unwrap();
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(lib_str)
    .bind(lib_str)
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(lib_str)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES (?, ?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(format!("{uuid}.cbz"))
    .bind(lib_id)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let entries: Vec<(String, Vec<u8>)> = (0..pages)
        .map(|i| (format!("p{i}.jpg"), b"x".to_vec()))
        .collect();
    let entry_refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let bytes = build_stored_zip(&entry_refs);
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'CBZ', ?, ?)",
    )
    .bind(book_id)
    .bind(uuid)
    .bind(bytes.len() as i64)
    .execute(pool)
    .await
    .unwrap();
    std::fs::write(dir.join(format!("{uuid}.cbz")), &bytes).unwrap();
    book_id
}

async fn page_count_of(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn backfill_page_counts_fills_null_rows_from_the_cbz_archive() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill");
    let a = seed_cbz_missing_page_count(&pool, &dir, "uuid-a", 3).await;
    let b = seed_cbz_missing_page_count(&pool, &dir, "uuid-b", 7).await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_page_counts(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(page_count_of(&pool, a).await, Some(3));
    assert_eq!(page_count_of(&pool, b).await, Some(7));
    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_page_counts_is_idempotent_once_filled() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill-idempotent");
    seed_cbz_missing_page_count(&pool, &dir, "uuid-a", 3).await;

    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();

    // Second pass: every book now has a count, so there are no candidates
    // and `on_progress` never fires.
    let mut calls = 0u32;
    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_page_counts_is_a_noop_when_no_candidates_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill-empty");
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind(dir.to_str().unwrap())
        .bind(dir.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let mut calls = 0u32;
    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- thumbnail backfill (#1752 / #1817 phantom-progress fix) ----------

/// Seed a `has_cover = 1` book with a real cover file on disk, so
/// `backfill_thumbs` has bytes to (maybe) re-encode. `last_modified` is
/// pinned far in the past so any thumbnail written just now is unambiguously
/// fresher than it, regardless of clock resolution.
async fn seed_covered_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str) -> i64 {
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, has_cover, last_modified) \
         VALUES (?, ?, ?, '', ?, ?, 1, 1) RETURNING id",
    )
    .bind(uuid)
    .bind(uuid)
    .bind(lib_id)
    .bind(title)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    std::fs::write(
        crate::covers_dir().join(format!("{uuid}.png")),
        crate::ebook::test_support::solid_color_png(200, 40, 40, 8, 8),
    )
    .unwrap();
    book_id
}

/// Write sentinel bytes (never produced by a real encode) at all three
/// thumbnail sizes for `book_id`, so it reads as fresh relative to its
/// far-past `last_modified` and a would-be re-encode is detectable.
fn write_fresh_sentinel_thumbs(book_id: i64) {
    let sentinel = b"not-a-real-webp-sentinel".to_vec();
    for size in crate::thumbs::ThumbSize::all() {
        std::fs::write(crate::thumbs::thumb_path_for(book_id, size), &sentinel).unwrap();
    }
}

/// AC1: a library whose covers are all already fresh posts no visible
/// progress — `on_progress` never fires and `backfill_thumbs` returns
/// without ever calling into the encode path.
#[tokio::test]
async fn backfill_thumbs_reports_no_progress_when_every_cover_is_already_fresh() {
    let covers_dir = make_test_dir("thumb-backfill-all-fresh-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-all-fresh-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-all-fresh-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let a = seed_covered_book(&pool, lib_id, "uuid-fresh-a", "Fresh A").await;
    let b = seed_covered_book(&pool, lib_id, "uuid-fresh-b", "Fresh B").await;
    write_fresh_sentinel_thumbs(a);
    write_fresh_sentinel_thumbs(b);

    let mut calls = 0u32;
    backfill_thumbs(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();

    assert_eq!(
        calls, 0,
        "on_progress must not fire when every candidate is already fresh"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}

/// AC2: a library where every cover is stale reports `count = N` and calls
/// `on_progress` exactly N times with `total = N`.
#[tokio::test]
async fn backfill_thumbs_reports_count_matching_the_stale_set_when_all_covers_are_stale() {
    let covers_dir = make_test_dir("thumb-backfill-all-stale-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-all-stale-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-all-stale-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    // No thumbnails written on disk at all, so every size for every book is
    // stale (a missing thumbnail counts as stale).
    seed_covered_book(&pool, lib_id, "uuid-stale-a", "Stale A").await;
    seed_covered_book(&pool, lib_id, "uuid-stale-b", "Stale B").await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_thumbs(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}

/// AC4: a library with both fresh and stale covers reports progress only
/// for the stale ones — the mixed case the phantom-progress bug collapsed
/// into "report every book with a cover".
#[tokio::test]
async fn backfill_thumbs_reports_progress_only_for_stale_covers_in_a_mixed_library() {
    let covers_dir = make_test_dir("thumb-backfill-mixed-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-mixed-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-mixed-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let fresh = seed_covered_book(&pool, lib_id, "uuid-mixed-fresh", "Mixed Fresh").await;
    write_fresh_sentinel_thumbs(fresh);
    // No thumbnails written for this one, so it's the sole stale candidate.
    seed_covered_book(&pool, lib_id, "uuid-mixed-stale", "Mixed Stale").await;

    let mut titles: Vec<String> = Vec::new();
    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_thumbs(&pool, dir.to_str().unwrap(), |p, t, title| {
        progress.push((p, t));
        titles.push(title.to_string());
    })
    .await
    .unwrap();

    assert_eq!(
        progress,
        vec![(1, 1)],
        "total and processed must both reflect the single stale book, not the two candidates"
    );
    assert_eq!(
        titles,
        vec!["Mixed Stale".to_string()],
        "on_progress must not fire for the already-fresh book"
    );

    let sentinel = b"not-a-real-webp-sentinel".to_vec();
    for size in crate::thumbs::ThumbSize::all() {
        let on_disk = std::fs::read(crate::thumbs::thumb_path_for(fresh, size)).unwrap();
        assert_eq!(
            on_disk, sentinel,
            "the already-fresh book's thumbnail for size {size} must not be re-encoded"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}

// ---------- #1057: ReindexStats plumbed off a real scan ----------

/// `reindex` returns the ghost count and pre-scan file-backed total it
/// measured, not just `()` — the tally the worker projects into a
/// [`omnibus_shared::GhostFilesWarning`] on the wire.
#[tokio::test]
async fn reindex_returns_stats_with_the_removed_count_and_file_backed_total() {
    let _covers = CoversTempDir::new("reindex-stats");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-stats-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;
    seed_ebook_at(&pool, &lib_path, "c.epub", "Carmilla").await;
    std::fs::remove_file(lib.join("b.epub")).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.removed, 1, "exactly one book's file went missing");
    assert_eq!(
        stats.file_backed_total, 3,
        "the pre-scan file-backed total is measured before this scan's removal"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// `root_display_name` keeps the directory basename through trailing
/// separators (`Path::file_name` ignores them — see its std doc example
/// `/usr/bin/` → `bin`); only the degenerate roots `/` and a `..`-ending
/// path yield no name, and `display_item` then degrades to the bare
/// relative path rather than emitting a leading slash.
#[test]
fn root_display_name_survives_trailing_separators_and_degrades_for_rootless_paths() {
    assert_eq!(root_display_name("/mnt/media/books"), "books");
    assert_eq!(root_display_name("/mnt/media/books/"), "books");
    assert_eq!(root_display_name("/"), "");
    assert_eq!(
        display_item(&root_display_name("/mnt/media/books/"), "Author/Title.epub"),
        "books/Author/Title.epub"
    );
    assert_eq!(
        display_item(&root_display_name("/"), "Author/Title.epub"),
        "Author/Title.epub"
    );
}

/// The verbose progress stream (issue #1802): a walking-phase event opens
/// the scan, parse and sync events carry the diff's tallies plus the
/// current item, and every reported path is the library directory name
/// plus the relative path — never the absolute server path.
#[tokio::test]
async fn reindex_with_progress_reports_phases_tallies_and_relative_current_items() {
    let _covers = CoversTempDir::new("reindex-verbose");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-verbose-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    let root_name = lib.file_name().unwrap().to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;
    // A file on disk with no DB row — the New bucket the parse + sync
    // phases will report on.
    std::fs::write(lib.join("fresh.epub"), b"stub").unwrap();

    let updates: std::sync::Arc<std::sync::Mutex<Vec<ScanUpdate>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let updates_cb = updates.clone();
    reindex_with_progress(&pool, &lib_path, move |u| {
        updates_cb.lock().unwrap().push(u);
    })
    .await
    .unwrap();

    let updates = updates.lock().unwrap();
    assert_eq!(
        updates.first().and_then(|u| u.detail.phase.as_deref()),
        Some(PHASE_WALKING),
        "the walking phase must open the stream"
    );

    let parse_event = updates
        .iter()
        .find(|u| u.detail.phase.as_deref() == Some(PHASE_PARSING))
        .expect("a parse-phase event for the New file");
    let item = parse_event.detail.current_item.as_deref().unwrap();
    assert_eq!(item, format!("{root_name}/fresh.epub"));
    let tallies = parse_event
        .detail
        .tallies
        .expect("parse events carry tallies");
    assert_eq!(tallies.found, 3);
    assert_eq!(tallies.new, 1);
    assert_eq!(tallies.unchanged, 2);

    let sync_event = updates
        .iter()
        .find(|u| {
            u.detail.phase.as_deref() == Some(PHASE_SYNCING) && u.detail.current_item.is_some()
        })
        .expect("a sync-phase event naming the written book");
    assert_eq!(
        sync_event.detail.current_item.as_deref().unwrap(),
        format!("{root_name}/fresh.epub")
    );

    for u in updates.iter() {
        if let Some(item) = u.detail.current_item.as_deref() {
            assert!(
                !item.contains(&lib_path),
                "current_item leaked the absolute library path: {item}"
            );
        }
    }

    let _ = std::fs::remove_dir_all(&lib);
}

// ---------- #819: incomplete-enumeration data-loss guard ----------

/// Read a book's `is_missing_files` flag by its `scan_key` (the relative
/// path). Returns `None` if no such book row exists.
async fn is_missing_by_scan_key(pool: &SqlitePool, scan_key: &str) -> Option<i64> {
    sqlx::query_scalar("SELECT is_missing_files FROM books WHERE scan_key = ?")
        .bind(scan_key)
        .fetch_optional(pool)
        .await
        .unwrap()
}

/// Index one EPUB through the real `sync_books` write path at `library_path`
/// (a real on-disk dir), writing a matching stub file so a later `reindex`
/// re-finds it. `filename` is the library-relative path.
async fn seed_ebook_at(pool: &SqlitePool, library_path: &str, filename: &str, title: &str) {
    let abs = std::path::Path::new(library_path).join(filename);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, b"not a zip").unwrap();
    let (mtime, size) = {
        let meta = std::fs::metadata(&abs).unwrap();
        (
            meta.modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            meta.len() as i64,
        )
    };
    // Seed the row with the real stat so the reindex classifies it Unchanged
    // (not Changed) on a healthy pass.
    let mut book = indexed(filename, Some(title), &["Author"], &[], None, None);
    book.mtime_epoch = mtime;
    book.size_bytes = size;
    sync_books(
        pool,
        library_path,
        SyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

/// Drop all permissions on `dir` so `read_dir` fails with EACCES. Returns
/// `false` when the platform ignores the perm change (e.g. running as root
/// in a CI container), so the caller can skip the assertion rather than
/// report a false negative.
#[cfg(unix)]
fn make_unreadable(dir: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(dir).is_err() {
        return true;
    }
    // The platform ignored the perm change (e.g. running as root). Restore
    // read access before bailing so the caller's temp-dir cleanup doesn't
    // trip over a 0o000 directory.
    restore_readable(dir);
    false
}

#[cfg(unix)]
fn restore_readable(dir: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755));
}

/// Acceptance (a): a scan against a transiently-empty root leaves every
/// book's `is_missing_files` flag untouched. Reproduces the boot-race /
/// unmounted-NFS incident: the DB has file-backed books, but the root reads
/// empty this pass. Nothing may be flagged missing.
#[tokio::test]
async fn reindex_against_transiently_empty_root_leaves_is_missing_unchanged() {
    let _covers = CoversTempDir::new("empty-root-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("empty-root-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "a.epub", "Dracula").await;
    seed_ebook_at(&pool, &lib_path, "b.epub", "Frankenstein").await;

    // Simulate the mount vanishing: delete the files so the root reads
    // empty, but the DB still holds two file-backed books.
    std::fs::remove_file(lib.join("a.epub")).unwrap();
    std::fs::remove_file(lib.join("b.epub")).unwrap();

    reindex(&pool, &lib_path).await.unwrap();

    // Neither book was flagged missing — the empty read was distrusted.
    assert_eq!(
        is_missing_by_scan_key(&pool, "a.epub").await,
        Some(0),
        "a book under a transiently-empty root must not be flagged missing"
    );
    assert_eq!(is_missing_by_scan_key(&pool, "b.epub").await, Some(0));
    // Both `book_files` rows survive.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2,
        "book_files must survive a transiently-empty scan"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// Acceptance (a) + "merged_uuids sacred": an EACCES fault mid-scan must
/// leave both `is_missing_files` and `merged_uuids` unchanged — no curation
/// record is eroded as a side effect of a partial scan.
#[cfg(unix)]
#[tokio::test]
async fn reindex_with_unreadable_subdir_preserves_missing_flags_and_merged_uuids() {
    let _covers = CoversTempDir::new("eacces-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("eacces-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    // A readable book at the top, and two under a subdir we'll lock. One of
    // the locked pair is merged into the readable one, so its EPUB-format
    // `merged_uuids` row is the curation record we must protect.
    seed_ebook_at(&pool, &lib_path, "top.epub", "Top").await;
    seed_ebook_at(&pool, &lib_path, "locked/source.epub", "Source").await;
    seed_ebook_at(&pool, &lib_path, "locked/other.epub", "Other").await;

    let target = crate::test_support::uuid_by_scan_key(&pool, "top.epub").await;
    let source = crate::test_support::uuid_by_scan_key(&pool, "locked/source.epub").await;
    crate::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let merged_before = count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await;
    assert!(
        merged_before >= 1,
        "test precondition: the merge must record a merged_uuids row"
    );

    let locked = lib.join("locked");
    if !make_unreadable(&locked) {
        let _ = std::fs::remove_dir_all(&lib);
        return; // running as root — perms don't bite; skip
    }

    let result = reindex(&pool, &lib_path).await;

    restore_readable(&locked);

    result.expect("a partial scan must still succeed (it just skips removal)");

    // The un-enumerated books under `locked/` were NOT flagged missing.
    assert_eq!(
        is_missing_by_scan_key(&pool, "locked/other.epub").await,
        Some(0),
        "a book under an unreadable subdir must not be flagged missing"
    );
    // The merge's curation record survives untouched.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        merged_before,
        "a scan must never erode merged_uuids"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// Acceptance (c): forcing a full rescan (`last_indexed = 0`) while some
/// files are unreadable must not wipe `book_files` or resurrect merged-away
/// books. Even with the refresh window bypassed, the partial enumeration is
/// distrusted so the removal pass never runs.
#[cfg(unix)]
#[tokio::test]
async fn forced_full_rescan_with_unreadable_files_does_not_wipe_or_resurrect() {
    let _covers = CoversTempDir::new("forced-rescan-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("forced-rescan-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "keep.epub", "Keep").await;
    seed_ebook_at(&pool, &lib_path, "locked/source.epub", "Source").await;

    let target = crate::test_support::uuid_by_scan_key(&pool, "keep.epub").await;
    let source = crate::test_support::uuid_by_scan_key(&pool, "locked/source.epub").await;
    crate::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let books_before = count_rows(&pool, "SELECT COUNT(*) FROM books").await;
    let files_before = count_rows(&pool, "SELECT COUNT(*) FROM book_files").await;
    let merged_before = count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await;

    // Force a full rescan by resetting last_indexed to the epoch.
    sqlx::query("UPDATE scan_roots SET last_indexed = 0 WHERE path = ?")
        .bind(&lib_path)
        .execute(&pool)
        .await
        .unwrap();

    let locked = lib.join("locked");
    if !make_unreadable(&locked) {
        let _ = std::fs::remove_dir_all(&lib);
        return; // running as root — skip
    }

    let result = reindex(&pool, &lib_path).await;

    restore_readable(&locked);
    result.expect("forced rescan under a partial fault must still succeed");

    // Nothing wiped: the same book, file, and merge counts as before.
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        books_before,
        "a forced rescan under a fault must not delete book rows"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        files_before,
        "a forced rescan under a fault must not drop book_files"
    );
    // The merged-away book did not resurrect (merged_uuids intact, book count
    // unchanged — no duplicate reappeared).
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        merged_before,
        "the merge must survive a forced rescan under a fault"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

#[tokio::test]
async fn is_stale_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = is_stale(&pool, "/lib").await.unwrap_err();
    assert!(matches!(err, IndexerError::Db(_)));
}

// ---------- #1537: a multi-file book is diffed per file, not per book ----------

/// Seed two EPUBs through the real `sync_books` write path with distinct
/// on-disk stats, then merge the second into the first — the exact repro
/// shape from #1537 (a book left holding two same-format `book_files`
/// rows). Returns `(target_uuid, source_uuid)`.
async fn seed_merged_two_epub_book(pool: &SqlitePool) -> (String, String) {
    sync_books(
        pool,
        "/ebooks",
        SyncPlan {
            new_books: vec![indexed_with_stat("A/one.epub", Some("One"), 1000, 500)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let target = uuid_by_scan_key(pool, "A/one.epub").await;

    sync_books(
        pool,
        "/ebooks",
        SyncPlan {
            new_books: vec![indexed_with_stat("B/two.epub", Some("Two"), 2000, 100)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = uuid_by_scan_key(pool, "B/two.epub").await;

    crate::merge::merge_books(pool, &source, &target, None)
        .await
        .unwrap();
    (target, source)
}

/// Build the DB side of the diff exactly as `reindex` does for the ebook
/// pass: the anchor row(s) via `list_indexed_rows_for_formats`, plus every
/// attached file via `list_merged_rows_for_formats`.
async fn ebook_db_rows(pool: &SqlitePool, library_path: &str) -> Vec<IndexedRow> {
    let mut rows = list_indexed_rows_for_formats(pool, library_path, crate::ebook::EBOOK_FORMATS)
        .await
        .unwrap();
    rows.extend(
        list_merged_rows_for_formats(pool, library_path, crate::ebook::EBOOK_FORMATS)
            .await
            .unwrap(),
    );
    rows
}

#[tokio::test]
async fn multi_file_book_from_merge_classifies_each_file_independently_when_one_changes() {
    // AC3: modifying only the second file's stat must classify that file
    // Changed while the first stays Unchanged, and vice versa. Before the
    // fix, `list_indexed_rows_for_formats` aggregated MAX(mtime)/MAX(size)
    // across both `book_files` rows, so this book reclassified Changed
    // regardless of which file (or neither) actually moved.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (target, source) = seed_merged_two_epub_book(&pool).await;
    let db_rows = ebook_db_rows(&pool, "/ebooks").await;

    // Only the second file (`B/two.epub`) changed on disk.
    let disk_second_changed = vec![
        entry("A/one.epub", "A/one.epub", 1000, 500),
        entry("B/two.epub", "B/two.epub", 2000, 999),
    ];
    let diff = diff_library(&disk_second_changed, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(diff.unchanged, vec![target.clone()], "{diff:?}");
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].filename, "B/two.epub");

    // Only the first file (`A/one.epub`) changed on disk.
    let disk_first_changed = vec![
        entry("A/one.epub", "A/one.epub", 1000, 999),
        entry("B/two.epub", "B/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk_first_changed, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(diff.unchanged, vec![source], "{diff:?}");
    assert_eq!(diff.changed.len(), 1);
    assert_eq!(diff.changed[0].filename, "A/one.epub");
}

/// AC1 + AC2: a book merged from two same-format EPUBs classifies both
/// files Unchanged on a rescan where neither changed, and stays that way
/// across three consecutive full reindexes — no re-parse, so each file's
/// `book_files.id` (and the book's `last_modified`) survives unchanged.
/// A Changed rewrite always mints a fresh `book_files` row (DELETE +
/// INSERT), so a stable id is direct evidence no re-parse happened.
#[tokio::test]
async fn merged_two_file_book_stays_unchanged_across_three_consecutive_reindexes() {
    let _covers = CoversTempDir::new("merge-reparse-guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("merge-reparse-guard-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    seed_ebook_at(&pool, &lib_path, "A/one.epub", "One").await;
    seed_ebook_at(&pool, &lib_path, "B/two.epub", "Two").await;
    let target = uuid_by_scan_key(&pool, "A/one.epub").await;
    let source = uuid_by_scan_key(&pool, "B/two.epub").await;
    crate::merge::merge_books(&pool, &source, &target, None)
        .await
        .unwrap();

    let file_ids_before: Vec<i64> = sqlx::query_scalar("SELECT id FROM book_files ORDER BY id")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        file_ids_before.len(),
        2,
        "test precondition: two files merged"
    );
    let last_modified_before: i64 =
        sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
            .bind(&target)
            .fetch_one(&pool)
            .await
            .unwrap();

    for pass in 1..=3 {
        reindex(&pool, &lib_path).await.unwrap();
        let file_ids_after: Vec<i64> = sqlx::query_scalar("SELECT id FROM book_files ORDER BY id")
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(
            file_ids_after, file_ids_before,
            "reindex pass {pass} re-wrote a book_files row (re-parsed) \
             instead of classifying Unchanged"
        );
        let last_modified_after: i64 =
            sqlx::query_scalar("SELECT last_modified FROM books WHERE uuid = ?")
                .bind(&target)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            last_modified_after, last_modified_before,
            "reindex pass {pass} touched books.last_modified (implies a Changed rewrite)"
        );
    }

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC6: an audiobook naturally holding two different-format groups under
/// one book (auto-attached by title+author match — not a manual merge)
/// classifies both groups Unchanged on a rescan where neither changed.
#[tokio::test]
async fn mixed_format_audiobook_group_classifies_both_parts_unchanged_when_nothing_changed() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    let mut m4b = indexed_audiobook("A/Dracula.m4b", "Dracula", Some("Stoker"));
    m4b.max_mtime_epoch = 1000;
    m4b.total_size_bytes = 500;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![m4b],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let target = uuid_by_scan_key(&pool, "A/Dracula.m4b").await;

    // Same title+author, different format — auto-attaches to the M4B book
    // instead of minting a second `books` row.
    let mut mp3 = indexed_audiobook("B/Dracula mp3", "Dracula", Some("Stoker"));
    mp3.format = "MP3".into();
    mp3.max_mtime_epoch = 2000;
    mp3.total_size_bytes = 100;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "test precondition: the MP3 group auto-attached rather than minting a new book"
    );
    let source = crate::test_support::uuid_by_scan_key(&pool, "B/Dracula mp3").await;
    assert_ne!(
        target, source,
        "the attached group keeps its own ledger uuid"
    );

    let disk = vec![
        entry("A/Dracula.m4b", "A/Dracula.m4b", 1000, 500),
        entry("B/Dracula mp3", "B/Dracula mp3", 2000, 100),
    ];
    let mut db_rows =
        list_indexed_rows_for_formats(&pool, "/audio", crate::audiobook::AUDIOBOOK_FORMATS)
            .await
            .unwrap();
    db_rows.extend(
        list_merged_rows_for_formats(&pool, "/audio", crate::audiobook::AUDIOBOOK_FORMATS)
            .await
            .unwrap(),
    );

    let diff = diff_library(&disk, &db_rows, Path::new("/audio"), true);
    assert_eq!(diff.unchanged.len(), 2, "{diff:?}");
    assert!(diff.unchanged.contains(&target));
    assert!(diff.unchanged.contains(&source));
    assert!(diff.new.is_empty(), "{:?}", diff.new);
    assert!(diff.changed.is_empty(), "{:?}", diff.changed);
}

// ---------- #1536: relocation, end-to-end through `reindex` ----------

/// Index one stub EPUB of a caller-chosen byte length at `library_path`,
/// seeding the row with the file's real stat so a later `reindex`
/// classifies it Unchanged on a healthy pass. Distinct lengths give each
/// book a distinct `(size, mtime)` pair — the relocation detector's primary
/// uniqueness path, rather than its stem tiebreaker.
async fn seed_sized_ebook_at(pool: &SqlitePool, library_path: &str, filename: &str, len: usize) {
    let abs = std::path::Path::new(library_path).join(filename);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&abs, vec![b'x'; len]).unwrap();
    let meta = std::fs::metadata(&abs).unwrap();
    let mut book = indexed(filename, Some("Title"), &["Author"], &[], None, None);
    book.mtime_epoch = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    book.size_bytes = meta.len() as i64;
    sync_books(
        pool,
        library_path,
        SyncPlan {
            new_books: vec![book],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

/// AC3, end-to-end: reorganizing a whole library on disk — far past both
/// `MASS_MISSING_MIN_ABSOLUTE` and `MASS_MISSING_FRACTION` — no longer
/// aborts the reindex, because every file lands in Moved instead of
/// Removed. Before this detector the same scan was a hard
/// `MassMissingError`, which is why the title+author path never got the
/// chance to recover it.
#[tokio::test]
async fn reindex_recovers_a_whole_library_reorganization_without_tripping_the_breaker() {
    let _covers = CoversTempDir::new("reindex-reorg");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-reorg-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    // Enough books to clear the absolute floor, so the percentage guard is
    // actually reachable — below it the scan would be waved through for a
    // reason that has nothing to do with move detection, and the test would
    // prove nothing. A `const` block so the precondition is a compile error.
    const BOOKS: usize = 12;
    const { assert!(BOOKS > MASS_MISSING_MIN_ABSOLUTE) };
    for i in 0..BOOKS {
        seed_sized_ebook_at(&pool, &lib_path, &format!("Old/{i}.epub"), 100 + i * 7).await;
    }
    let before: Vec<(String, String)> =
        sqlx::query_as("SELECT scan_key, uuid FROM books ORDER BY scan_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(before.len(), BOOKS);

    // Reorganize every file into a different directory, preserving bytes
    // and timestamps — what `mv` does within one filesystem.
    std::fs::create_dir_all(lib.join("New")).unwrap();
    for i in 0..BOOKS {
        std::fs::rename(
            lib.join(format!("Old/{i}.epub")),
            lib.join(format!("New/{i}.epub")),
        )
        .unwrap();
    }
    std::fs::remove_dir(lib.join("Old")).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.moved, BOOKS, "AC3: every file was matched as moved");
    assert_eq!(
        stats.removed, 0,
        "AC3: nothing ghosted, so nothing to abort"
    );
    assert_eq!(stats.ghost_warning(), None, "a move is not a ghost");

    let after: Vec<(String, String)> =
        sqlx::query_as("SELECT scan_key, uuid FROM books ORDER BY scan_key")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(after.len(), BOOKS, "no duplicates were inserted");
    for ((old_key, old_uuid), (new_key, new_uuid)) in before.iter().zip(after.iter()) {
        assert_eq!(old_key.replace("Old/", "New/"), *new_key);
        assert_eq!(old_uuid, new_uuid, "identity rode along with the move");
    }
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM merged_uuids").await,
        0,
        "AC1: no ledger rows minted for same-format relocations"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books WHERE is_missing_files = 1"
        )
        .await,
        0,
        "no book was flagged missing"
    );
    // Every file row followed its book.
    let stale: i64 = count_rows(
        &pool,
        "SELECT COUNT(*) FROM book_files WHERE scan_key NOT LIKE 'New/%'",
    )
    .await;
    assert_eq!(
        stale, 0,
        "AC2: every book_files.scan_key names the new path"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// The same library, with the same reorganization, but one file's bytes
/// changed on the way: it falls out of Moved into Removed + New (AC6)
/// while its neighbours still relocate.
#[tokio::test]
async fn reindex_leaves_a_relocation_that_also_changed_bytes_in_removed_and_new() {
    let _covers = CoversTempDir::new("reindex-reorg-edited");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-reorg-edited-lib");
    let lib_path = lib.to_string_lossy().into_owned();

    for i in 0..3 {
        seed_sized_ebook_at(&pool, &lib_path, &format!("Old/{i}.epub"), 100 + i * 7).await;
    }
    std::fs::create_dir_all(lib.join("New")).unwrap();
    for i in 0..3 {
        std::fs::rename(
            lib.join(format!("Old/{i}.epub")),
            lib.join(format!("New/{i}.epub")),
        )
        .unwrap();
    }
    // Rewrite one file at a different length — its stat pair no longer
    // matches the row it came from.
    std::fs::write(lib.join("New/1.epub"), vec![b'y'; 4096]).unwrap();

    let stats = reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(stats.moved, 2, "AC6: only the untouched files relocated");
    assert_eq!(stats.removed, 1, "AC6: the edited file's old row ghosted");

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC8, at the classifier: for a book holding two EPUBs (via
/// `merge_books`), moving **either** file is detected independently.
/// Candidacy is deliberately not gated on the book's file count — that
/// would silently exclude the anchor file of every merged book, which is
/// exactly the case this detector most needs to handle.
#[tokio::test]
async fn each_file_of_a_merged_two_epub_book_is_independently_move_matched() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let (target, source) = seed_merged_two_epub_book(&pool).await;
    let db_rows = ebook_db_rows(&pool, "/ebooks").await;

    // The anchor file — the one `books.scan_key` names — moved.
    let disk = vec![
        entry("Moved/one.epub", "Moved/one.epub", 1000, 500),
        entry("B/two.epub", "B/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(
        diff.moved,
        vec![crate::sync::MovedFile {
            uuid: target.clone(),
            filename: "Moved/one.epub".into(),
        }],
        "the anchor of a merged book is move-matched by its own books.uuid: {diff:?}"
    );
    assert_eq!(
        diff.unchanged,
        vec![source.clone()],
        "the sibling is untouched"
    );
    assert!(diff.removed.is_empty());
    assert!(diff.new.is_empty());

    // The merged-in file moved instead — matched by its ledger uuid.
    let disk = vec![
        entry("A/one.epub", "A/one.epub", 1000, 500),
        entry("Moved/two.epub", "Moved/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    assert_eq!(
        diff.moved,
        vec![crate::sync::MovedFile {
            uuid: source.clone(),
            filename: "Moved/two.epub".into(),
        }],
        "the merged-in file is move-matched by its merged_uuids handle: {diff:?}"
    );
    assert_eq!(diff.unchanged, vec![target.clone()]);

    // Both moved in one scan — their stat pairs differ, so both land.
    let disk = vec![
        entry("Moved/one.epub", "Moved/one.epub", 1000, 500),
        entry("Moved/two.epub", "Moved/two.epub", 2000, 100),
    ];
    let diff = diff_library(&disk, &db_rows, Path::new("/ebooks"), true);
    let mut moved: Vec<(String, String)> = diff
        .moved
        .iter()
        .map(|m| (m.uuid.clone(), m.filename.clone()))
        .collect();
    moved.sort();
    let mut expected = vec![
        (target, "Moved/one.epub".to_string()),
        (source, "Moved/two.epub".to_string()),
    ];
    expected.sort();
    assert_eq!(moved, expected);
    assert!(diff.removed.is_empty());
    assert!(diff.new.is_empty());
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

// ---------- #1562: CBZ ingestion through the normal diff/sync path ----------

/// Write a minimal real CBZ (ComicInfo.xml + one PNG page) at
/// `library_path/filename` so `reindex` exercises the actual comic parser.
fn write_cbz_at(library_path: &str, filename: &str) {
    let comic_info: &[u8] = br#"<ComicInfo>
  <Title>The Longing</Title>
  <Series>Berserk</Series>
  <Number>3</Number>
  <Writer>Kentaro Miura</Writer>
</ComicInfo>"#;
    let img = image::RgbImage::from_pixel(4, 4, image::Rgb([200, 60, 50]));
    let mut png = Vec::new();
    image::DynamicImage::ImageRgb8(img)
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .expect("png encode");
    let bytes =
        crate::test_support::build_stored_zip(&[("ComicInfo.xml", comic_info), ("p001.png", &png)]);
    std::fs::write(std::path::Path::new(library_path).join(filename), bytes).unwrap();
}

/// AC1: a CBZ in the ebook library indexes as a book with ComicInfo
/// title/author metadata, a first-page cover, and format `CBZ`.
#[tokio::test]
async fn reindex_indexes_a_cbz_with_comic_info_metadata_cover_and_cbz_format() {
    let _covers = CoversTempDir::new("reindex-cbz");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "berserk-v03.cbz");

    reindex(&pool, &lib_path).await.unwrap();

    let (title, has_cover): (String, i64) =
        sqlx::query_as("SELECT title, has_cover FROM books WHERE scan_key = 'berserk-v03.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "The Longing");
    assert_eq!(has_cover, 1, "the first page becomes the cover");
    let format: String = sqlx::query_scalar(
        "SELECT bf.format FROM book_files bf
          JOIN books b ON b.id = bf.book_id WHERE b.scan_key = 'berserk-v03.cbz'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(format, "CBZ");
    let author: String = sqlx::query_scalar(
        "SELECT a.name FROM authors a
          JOIN books_authors_link l ON l.author = a.id
          JOIN books b ON b.id = l.book WHERE b.scan_key = 'berserk-v03.cbz'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(author, "Kentaro Miura");

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC2: reindex classifies an unchanged CBZ Unchanged (uuid preserved),
/// and removing the file ghosts the book — `book_files` dropped, the
/// `books` row (and its uuid) retained.
#[tokio::test]
async fn reindex_preserves_cbz_uuid_and_ghosts_the_book_when_the_file_is_removed() {
    let _covers = CoversTempDir::new("reindex-cbz-ghost");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-ghost-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "berserk-v03.cbz");

    reindex(&pool, &lib_path).await.unwrap();
    let uuid = uuid_by_scan_key(&pool, "berserk-v03.cbz").await;

    reindex(&pool, &lib_path).await.unwrap();
    assert_eq!(
        uuid_by_scan_key(&pool, "berserk-v03.cbz").await,
        uuid,
        "a second scan preserves books.uuid"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        1,
        "the unchanged CBZ is not re-inserted"
    );

    std::fs::remove_file(lib.join("berserk-v03.cbz")).unwrap();
    reindex(&pool, &lib_path).await.unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0,
        "the removed file's book_files row is dropped"
    );
    assert_eq!(
        uuid_by_scan_key(&pool, "berserk-v03.cbz").await,
        uuid,
        "the ghosted books row keeps its identity"
    );

    let _ = std::fs::remove_dir_all(&lib);
}

/// AC3: a malformed CBZ (not a zip) degrades to a metadata-less book row —
/// filename-fallback title, no cover — and the scan completes. (The parse
/// error itself lives on the in-memory `IndexedBook`, pinned by the
/// `comic::tests` failure-mode tests; the `books` schema has no error
/// column to persist it.)
#[tokio::test]
async fn reindex_surfaces_a_malformed_cbz_as_an_error_row_without_aborting() {
    let _covers = CoversTempDir::new("reindex-cbz-bad");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = make_test_dir("reindex-cbz-bad-lib");
    let lib_path = lib.to_string_lossy().into_owned();
    write_cbz_at(&lib_path, "good.cbz");
    std::fs::write(lib.join("bad.cbz"), b"not a zip").unwrap();

    reindex(&pool, &lib_path).await.unwrap();

    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        2,
        "the malformed archive does not abort the scan"
    );
    let good_title: String =
        sqlx::query_scalar("SELECT title FROM books WHERE scan_key = 'good.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(good_title, "The Longing", "the healthy CBZ still indexes");
    let (bad_title, bad_cover): (String, i64) =
        sqlx::query_as("SELECT title, has_cover FROM books WHERE scan_key = 'bad.cbz'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        bad_title, "bad.cbz",
        "the error row falls back to the filename title"
    );
    assert_eq!(bad_cover, 0, "the error row carries no cover");

    let _ = std::fs::remove_dir_all(&lib);
}
