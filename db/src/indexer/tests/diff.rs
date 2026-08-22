//! `diff_library` bucket classification: new / removed / unchanged /
//! changed / backfill, the fileless-book cases, and what an untrustworthy
//! enumeration suppresses.

use crate::books::IndexedRow;
use crate::ebook::StatEntry;

use super::super::*;
use super::{entry, fileless_row, row};

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
