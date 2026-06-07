//! Background ebook indexing (server-only).
//!
//! The web and mobile list endpoints read from the `books` table instead of
//! walking the filesystem on every request. This module owns the write side:
//! scan the configured library, diff the result against the DB, and apply
//! only the per-book changes that the on-disk state demands.
//!
//! Two triggers fire a reindex (both routed through
//! [`crate::worker::Worker`] so concurrency and per-path serialization are
//! enforced centrally):
//! - On startup, if no index exists yet or the existing one is older than
//!   [`REFRESH_AFTER_SECS`].
//! - On every settings save (the library path may have changed, and even if
//!   it didn't the user likely just added or removed books).
//!
//! Scans run on the blocking pool via `spawn_blocking` so the hot axum
//! runtime stays responsive while the walk + OPF parse + cover reads go.
//!
//! ## Diff classification
//!
//! [`diff_library`] takes the Phase-A stat output and the DB's current
//! state and buckets each file:
//!
//! - **Unchanged** — on disk, in DB, `(mtime_epoch, size_bytes)` matches.
//!   No work. `books.id` preserved.
//! - **New** — on disk, not in DB. Full Phase-B parse + insert.
//! - **Changed** — on disk, in DB, stat differs. Full Phase-B parse, then
//!   UPDATE in place (preserves `books.id`).
//! - **Removed** — in DB, not on disk. DELETE (cascades clean).
//! - **Backfill** — in DB, in disk, DB has the migration default
//!   `(mtime_epoch=0, size_bytes=0)`. Treated as the sentinel for "fs
//!   metadata never observed" (post-migration), so the writer only
//!   updates the stat columns; the OPF is not re-parsed. Without this,
//!   the first reindex after the migration would treat every existing
//!   row as Changed.

use std::path::{Path, PathBuf};

use sqlx::SqlitePool;

use crate::{audiobook, books, ebook, sync};

/// Errors returned by the pure-DB indexer reads (`is_stale`). The
/// transparent `Db` variant honors the `02-error-handling` boundary rule
/// — no raw `sqlx::Error` across the module boundary — while keeping the
/// `?` propagation clean. `reindex` and `reindex_audiobooks` stay on
/// `anyhow::Result` because they span filesystem scans + parsing
/// (foreign-system failure space).
#[derive(Debug, thiserror::Error)]
pub enum IndexerError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Reindex if the last successful index is older than this. One hour is a
/// compromise between responsiveness to on-disk changes and avoiding
/// thrashing the disk for users who leave the app open all day.
pub const REFRESH_AFTER_SECS: i64 = 60 * 60;

/// True when a refresh should be kicked off: no state at all, or state
/// older than [`REFRESH_AFTER_SECS`].
///
/// ## Clock-failure behavior
///
/// The freshness check is `now - last_indexed >= REFRESH_AFTER_SECS`. If
/// reading the wall clock fails (`SystemTime::now()` is before the UNIX
/// epoch — only reachable with a badly misconfigured system clock), the
/// `.unwrap_or(last)` fallback substitutes the stored `last_indexed`
/// timestamp for `now`. That makes `now - last == 0`, so the function
/// returns `Ok(false)` and the reindex is **silently skipped**.
///
/// This is a deliberate "serve stale rather than thrash the disk"
/// tradeoff: when the clock is unreadable we can't tell how old the index
/// is, so rather than re-scanning the library on every poll (a clock that
/// stays broken would otherwise trigger a reindex on every call) we keep
/// serving the existing index until the clock recovers. The decision is
/// factored into the pure [`is_stale_decision`] so the window boundaries
/// and this clock-failure fallback (`now == last`) are pinned by the
/// `is_stale_decision_*` tests below; change it only with intent.
pub async fn is_stale(pool: &SqlitePool, library_path: &str) -> Result<bool, IndexerError> {
    let Some(last) = crate::settings::last_indexed_at(pool, library_path).await? else {
        return Ok(true);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        // Clock unreadable: substitute `last` so `now - last == 0` and we
        // serve stale (see the doc comment above).
        .unwrap_or(last);
    Ok(is_stale_decision(last, now))
}

/// Pure freshness decision used by [`is_stale`]: stale when the index is at
/// least [`REFRESH_AFTER_SECS`] old. Factored out so the window boundaries
/// and the clock-failure fallback (which calls this with `now == last`) are
/// unit-testable without depending on a readable wall clock.
fn is_stale_decision(last: i64, now: i64) -> bool {
    now - last >= REFRESH_AFTER_SECS
}

/// Result of [`diff_library`]. Each bucket is what the writer should do
/// for the corresponding subset of files.
#[derive(Debug, Default)]
pub struct ReindexDiff {
    /// Already in the DB and the on-disk stat matches. Writer does
    /// nothing — `books.id` is preserved by definition.
    pub unchanged: Vec<String>,
    /// Not in the DB. Writer parses + inserts.
    pub new: Vec<ebook::ParseTarget>,
    /// In the DB but the stat differs. Writer parses + UPDATEs in place
    /// (`books.id` preserved).
    pub changed: Vec<ebook::ParseTarget>,
    /// In the DB but not on disk. Writer deletes; cascades clear the link
    /// tables; cover files on disk get cleaned up post-commit.
    pub removed: Vec<String>,
    /// In the DB and on disk, but the DB row carries the post-migration
    /// `(mtime_epoch=0, size_bytes=0)` sentinel. Writer fills in the stat
    /// values without re-parsing the OPF — the one-time upgrade fast path.
    pub backfill: Vec<(String, i64, i64)>,
}

/// Pure classifier — no I/O. Compares a Phase-A stat output against the
/// current DB state and routes each file into one of the five buckets.
/// `library_root` is the absolute path the scanner walked; we join it
/// with each `filename` to fill `ParseTarget.absolute` so Phase B can
/// open files directly without re-walking.
pub fn diff_library(
    disk: &[ebook::StatEntry],
    db: &[books::IndexedRow],
    library_root: &Path,
) -> ReindexDiff {
    use std::collections::HashMap;

    // Skip the synthetic "unreadable subdir" placeholders the stat walk
    // emits with an empty uuid (`scan_ebook_library_with` lifts those
    // into error rows for the legacy wrapper, but the diff treats them
    // as not-present).
    let disk_by_uuid: HashMap<&str, &ebook::StatEntry> = disk
        .iter()
        .filter(|e| !e.uuid.is_empty())
        .map(|e| (e.uuid.as_str(), e))
        .collect();
    let db_by_uuid: HashMap<&str, &books::IndexedRow> =
        db.iter().map(|r| (r.uuid.as_str(), r)).collect();

    let mut out = ReindexDiff::default();

    for (uuid, entry) in &disk_by_uuid {
        match db_by_uuid.get(uuid) {
            None => out.new.push(ebook::ParseTarget {
                filename: entry.filename.clone(),
                absolute: library_root.join(&entry.filename),
                mtime_epoch: entry.mtime_epoch,
                size_bytes: entry.size_bytes,
            }),
            Some(row) => {
                let never_observed = row.mtime_epoch == 0 && row.size_bytes == 0;
                let matches =
                    row.mtime_epoch == entry.mtime_epoch && row.size_bytes == entry.size_bytes;
                if never_observed {
                    out.backfill
                        .push(((*uuid).to_string(), entry.mtime_epoch, entry.size_bytes));
                } else if matches {
                    out.unchanged.push((*uuid).to_string());
                } else {
                    out.changed.push(ebook::ParseTarget {
                        filename: entry.filename.clone(),
                        absolute: library_root.join(&entry.filename),
                        mtime_epoch: entry.mtime_epoch,
                        size_bytes: entry.size_bytes,
                    });
                }
            }
        }
    }

    for row in db {
        if !disk_by_uuid.contains_key(row.uuid.as_str()) {
            out.removed.push(row.uuid.clone());
        }
    }

    // Stable order keeps the writer's behavior predictable across runs
    // (matters for the cover-file post-commit step, and for tests).
    out.new.sort_by(|a, b| a.filename.cmp(&b.filename));
    out.changed.sort_by(|a, b| a.filename.cmp(&b.filename));
    out.unchanged.sort();
    out.removed.sort();
    out.backfill.sort_by(|a, b| a.0.cmp(&b.0));

    out
}

/// Scan `library_path`, diff against the existing index, and apply only
/// the per-book changes the diff demands. Runs the scan on the blocking
/// pool so callers can `await` it from a normal async context without
/// blocking the runtime.
///
/// A fatal scan error (missing or unreadable root) is returned as `Err`
/// and the existing index is **not** touched — we'd rather serve
/// stale-but-good data than wipe the table and mark the index "fresh"
/// (which would also suppress retries until [`REFRESH_AFTER_SECS`]
/// elapses). Per-book parse failures are *not* fatal; they land in the
/// DB as rows with `error = Some(_)`, same as before.
pub async fn reindex(pool: &SqlitePool, library_path: &str) -> anyhow::Result<()> {
    let path_for_scan = library_path.to_owned();
    let library_key_for_scan = library_path.to_owned();
    let stat = tokio::task::spawn_blocking(move || {
        ebook::stat_ebook_library(Some(&path_for_scan), &library_key_for_scan)
    })
    .await?;
    if let Some(msg) = stat.error {
        anyhow::bail!("scan of {library_path} failed: {msg}");
    }

    // Scope to ebook formats so a shared ebook + audiobook library_path
    // does not classify audiobook rows here as Removed (#328 inverse).
    let db_rows =
        books::list_indexed_rows_for_formats(pool, library_path, ebook::EBOOK_FORMATS).await?;
    let library_root: PathBuf = PathBuf::from(library_path);
    let diff = diff_library(&stat.entries, &db_rows, &library_root);

    // Parse Phase B only for the buckets that need it.
    let new_targets = diff.new.clone();
    let changed_targets = diff.changed.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        // Materialize cover sidecars so future scans skip the zip
        // (F0.6). Best-effort: read-only filesystems fall through to the
        // in-memory bytes for the current scan and retry next time.
        let opts = ebook::ScanOptions {
            materialize_sidecars: true,
        };
        let new_books = ebook::parse_ebook_targets(new_targets, opts.clone());
        let changed_books = ebook::parse_ebook_targets(changed_targets, opts);
        (new_books, changed_books)
    })
    .await?;

    let plan = sync::SyncPlan {
        new_books: parsed.0,
        changed_books: parsed.1,
        removed_uuids: diff.removed,
        backfill: diff.backfill,
    };
    sync::sync_books(pool, library_path, plan).await?;
    Ok(())
}

/// F2.3 sibling of [`reindex`] for the audiobook library. Uses Phase A.5
/// group-by-folder, Phase B multi-part tag-reads, and [`sync::sync_audiobooks`]
/// which writes `book_file_parts` rows for the HLS pipeline.
pub async fn reindex_audiobooks(pool: &SqlitePool, library_path: &str) -> anyhow::Result<()> {
    // Phase A: stat every audio file.
    let path_for_scan = library_path.to_owned();
    let library_key = library_path.to_owned();
    let stat = tokio::task::spawn_blocking(move || {
        audiobook::stat_audiobook_library(Some(&path_for_scan), &library_key)
    })
    .await?;
    if let Some(msg) = stat.error {
        anyhow::bail!("audiobook scan of {library_path} failed: {msg}");
    }

    // Phase A.5: group per-file entries into one AudiobookGroup per book.
    let entries = stat.entries;
    let library_key2 = library_path.to_owned();
    let groups =
        tokio::task::spawn_blocking(move || audiobook::group_into_books(entries, &library_key2))
            .await?;

    // Diff groups against DB rows (project groups to the ebook StatEntry shape
    // so diff_library can be reused verbatim). Scope to audiobook formats so a
    // shared ebook + audiobook library_path does not classify EPUB rows here
    // as Removed (#328).
    let db_rows =
        books::list_indexed_rows_for_formats(pool, library_path, audiobook::AUDIOBOOK_FORMATS)
            .await?;
    let library_root: PathBuf = PathBuf::from(library_path);
    let groups_as_stat: Vec<ebook::StatEntry> = groups
        .iter()
        .filter(|g| !g.uuid.is_empty())
        .map(|g| ebook::StatEntry {
            filename: g.group_path.clone(),
            uuid: g.uuid.clone(),
            mtime_epoch: g.max_mtime_epoch,
            size_bytes: g.total_size_bytes,
            error: None,
        })
        .collect();
    let diff = diff_library(&groups_as_stat, &db_rows, &library_root);

    // Phase B: parse only the New and Changed groups.
    let groups_by_group_path: std::collections::HashMap<String, audiobook::AudiobookGroup> = groups
        .into_iter()
        .filter(|g| !g.uuid.is_empty())
        .map(|g| (g.group_path.clone(), g))
        .collect();

    let new_groups: Vec<audiobook::AudiobookGroup> = diff
        .new
        .iter()
        .filter_map(|t| groups_by_group_path.get(&t.filename).cloned())
        .collect();
    let changed_groups: Vec<audiobook::AudiobookGroup> = diff
        .changed
        .iter()
        .filter_map(|t| groups_by_group_path.get(&t.filename).cloned())
        .collect();

    let root_for_parse = library_root.clone();
    let parsed = tokio::task::spawn_blocking(move || {
        let new_books = audiobook::parse_groups(new_groups, &root_for_parse);
        let changed_books = audiobook::parse_groups(changed_groups, &root_for_parse);
        (new_books, changed_books)
    })
    .await?;

    let plan = sync::AudiobookSyncPlan {
        new_books: parsed.0,
        changed_books: parsed.1,
        removed_uuids: diff.removed,
        backfill: diff.backfill,
    };
    sync::sync_audiobooks(pool, library_path, plan).await?;
    Ok(())
}

/// Fill `file_chapters` for audiobook `book_files` rows that have none.
///
/// The chapter extraction pipeline was added after the initial audiobook
/// indexer, so books indexed before the migration have zero `file_chapters`
/// rows. The normal diff-based reindex skips unchanged files, so this
/// backfill runs as a separate worker task and is a no-op once all books
/// have chapters. `on_progress(processed, total)` is called after each
/// book so the UI can render a progress bar.
pub(crate) async fn backfill_chapters(
    pool: &SqlitePool,
    library_path: &str,
    on_progress: impl Fn(u32, u32),
) -> anyhow::Result<()> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT bf.id, bfp.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON bf.book_id = b.id \
         JOIN libraries l ON b.library_id = l.id \
         JOIN book_file_parts bfp ON bfp.book_file_id = bf.id \
         WHERE l.path = ? \
           AND bf.format IN ('M4B', 'M4A', 'MP3') \
           AND bfp.ordinal = 0 \
           AND NOT EXISTS (SELECT 1 FROM file_chapters fc WHERE fc.book_file_id = bf.id) \
         ORDER BY bf.id",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    if rows.is_empty() {
        return Ok(());
    }

    let total = rows.len() as u32;
    tracing::info!(
        count = total,
        "backfilling chapters for existing audiobooks"
    );

    let lib_root = PathBuf::from(library_path);
    for (i, (book_file_id, first_part_filename, format)) in rows.iter().enumerate() {
        let abs = lib_root.join(first_part_filename);
        let chapters = audiobook::extract_chapters(&abs, format);

        let parts: Vec<crate::audiobook::AudiobookPart> =
            sqlx::query_as::<_, (i64, String, i64, i64, f64)>(
                "SELECT ordinal, filename, size_bytes, mtime_epoch, duration_seconds \
                 FROM book_file_parts WHERE book_file_id = ? ORDER BY ordinal",
            )
            .bind(book_file_id)
            .fetch_all(pool)
            .await?
            .into_iter()
            .map(
                |(ordinal, filename, size_bytes, mtime_epoch, duration_seconds)| {
                    crate::audiobook::AudiobookPart {
                        ordinal,
                        filename,
                        size_bytes,
                        mtime_epoch,
                        duration_seconds,
                    }
                },
            )
            .collect();

        let mut tx = pool.begin().await?;
        sync::insert_chapters(&mut tx, *book_file_id, &chapters, &parts).await?;
        tx.commit().await?;

        on_progress(i as u32 + 1, total);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::{list_books, IndexedRow};
    use crate::ebook::StatEntry;
    use crate::pool::init_db;
    use crate::sync::replace_books;
    use crate::test_support::{indexed, CoversTempDir};

    /// Seed a `libraries` row for `path` with an explicit `last_indexed`
    /// epoch-seconds value. There's no public writer for `last_indexed`
    /// that lets a test set an arbitrary timestamp (`sync_books` always
    /// stamps "now"), so the `is_stale` window tests insert the row
    /// directly — exactly the columns `last_indexed_at` reads back.
    async fn seed_last_indexed(pool: &SqlitePool, path: &str, last_indexed: i64) {
        sqlx::query("INSERT INTO libraries (path, display_name, last_indexed) VALUES (?, ?, ?)")
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

    fn entry(name: &str, uuid: &str, mtime: i64, size: i64) -> StatEntry {
        StatEntry {
            filename: name.into(),
            uuid: uuid.into(),
            mtime_epoch: mtime,
            size_bytes: size,
            error: None,
        }
    }
    fn row(uuid: &str, mtime: i64, size: i64) -> IndexedRow {
        IndexedRow {
            uuid: uuid.into(),
            mtime_epoch: mtime,
            size_bytes: size,
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
                uuid: String::new(),
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
                uuid: "uuid-m4b".to_string(),
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
}
