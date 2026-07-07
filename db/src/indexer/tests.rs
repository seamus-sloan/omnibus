//! Unit tests for the `indexer` module — `diff_library` classifiers,
//! `is_stale` window logic, `reindex` preservation-on-failure, the
//! incomplete-enumeration data-loss guard (#819), and the shared-path
//! cross-format deletion guard.

use super::*;
use crate::books::{list_books, IndexedRow};
use crate::ebook::StatEntry;
use crate::pool::init_db;
use crate::sync::{replace_books, sync_books, SyncPlan};
use crate::test_support::{count_rows, indexed, make_test_dir, CoversTempDir};

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
/// A fileless book row (F2): retained book whose file is gone.
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
        changed_books: vec![],
        removed_uuids: vec![],
        backfill: vec![],
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
    backfill_chapters(&pool, lib, |processed, total| {
        progress_calls.push((processed, total));
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
    backfill_chapters(&pool, lib, |_, _| {
        progress_calls += 1;
    })
    .await
    .unwrap();

    assert_eq!(
        progress_calls, 0,
        "on_progress must not be called when all books already have chapters"
    );
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
