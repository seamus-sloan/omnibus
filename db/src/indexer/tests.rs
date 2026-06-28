//! Unit tests for the `indexer` module — `diff_library` classifiers,
//! `is_stale` window logic, `reindex` preservation-on-failure, and the
//! shared-path cross-format deletion guard.

use super::*;
use crate::books::{list_books, IndexedRow};
use crate::ebook::StatEntry;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

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
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
    assert_eq!(d.removed, vec!["uuid-a".to_string()]);
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
    assert!(d.unchanged.is_empty());
}

#[test]
fn diff_classifies_matching_stat_as_unchanged() {
    let disk = vec![entry("a.epub", "uuid-a", 100, 1000)];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
    assert_eq!(d.changed.len(), 1);
    assert_eq!(d.changed[0].mtime_epoch, 200);
    assert!(d.unchanged.is_empty());
}

#[test]
fn diff_classifies_size_drift_as_changed() {
    let disk = vec![entry("a.epub", "uuid-a", 100, 2000)];
    let db = vec![row("uuid-a", 100, 1000)];
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
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
    let d = diff_library(&disk, &db, Path::new("/lib"));
    assert!(d.removed.is_empty());
    assert!(d.new.is_empty());
    assert!(d.changed.is_empty());
}

#[test]
fn diff_builds_absolute_paths_for_parse_targets() {
    let disk = vec![entry("sub/a.epub", "uuid-a", 100, 1000)];
    let d = diff_library(&disk, &[], Path::new("/srv/library"));
    assert_eq!(d.new[0].absolute, Path::new("/srv/library/sub/a.epub"));
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

    // The audiobook reindex needs a real (but empty) directory to
    // scan — `stat_audiobook_library` returns an error otherwise.
    let shared = crate::test_support::make_test_dir("reindex-audio-shared-lib");
    let shared_path = shared.to_string_lossy().into_owned();

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

    // Run the audiobook reindex against the empty directory. The
    // audiobook row should be classified as Removed (and deleted);
    // the EPUB row must survive because its format is outside the
    // audiobook allow-list.
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

    // Run the ebook reindex against the empty directory. The EPUB
    // row should be classified as Removed (and deleted); the M4B
    // row must survive because its format is outside the ebook
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
