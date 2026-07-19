//! Background ebook indexing (server-only). Scans the configured library,
//! diffs the result against the `books` table, and applies only the
//! per-book changes the on-disk state demands. Reindex is driven by
//! [`crate::worker::Worker`] on startup (when stale) and on settings save;
//! scans run via `spawn_blocking` so the axum runtime stays responsive.

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

impl From<crate::settings::SettingsError> for IndexerError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => IndexerError::Db(inner),
            // `Validation`/`Json` are only produced by `set_hardcover_api_key`
            // and the F5.1 metadata-precedence writes, which no indexer path
            // calls — keep the arms exhaustive but do not widen
            // `IndexerError`'s surface for cases the caller graph can't reach.
            crate::settings::SettingsError::Validation(msg) => IndexerError::Db(
                sqlx::Error::Protocol(format!("unexpected settings validation error: {msg}")),
            ),
            crate::settings::SettingsError::Json(inner) => IndexerError::Db(sqlx::Error::Protocol(
                format!("unexpected settings JSON error: {inner}"),
            )),
        }
    }
}

/// Reindex if the last successful index is older than this. One hour is a
/// compromise between responsiveness to on-disk changes and avoiding
/// thrashing the disk for users who leave the app open all day.
pub const REFRESH_AFTER_SECS: i64 = 60 * 60;

/// Circuit-breaker threshold: abort the removal pass when a
/// single scan would flag more than this fraction of a previously-populated
/// library missing. A legitimate bulk delete this large is rare; a scan
/// that would erase a fifth of the library is far more likely a transient
/// mount/permission fault, so we halt and serve the existing index rather
/// than commit the wipe.
pub const MASS_MISSING_FRACTION: f64 = 0.20;

/// A scan that flags at most this many books missing is always allowed
/// through, regardless of [`MASS_MISSING_FRACTION`]. Without a floor, the
/// circuit-breaker would trip on ordinary small-library edits (deleting 1
/// of 3 books is 33%). Only libraries larger than this can trip the
/// percentage guard.
pub const MASS_MISSING_MIN_ABSOLUTE: usize = 10;

/// Warn threshold: below [`MASS_MISSING_FRACTION`] (the #819 abort guard)
/// but still large enough that a dropped mount or partial sync shouldn't
/// ghost books silently. Half the abort fraction — a scan in this band
/// still proceeds (unlike the abort guard) but its `Done` state carries a
/// dismissible warning (issue #1057).
pub const MASS_MISSING_WARN_FRACTION: f64 = 0.10;

/// Error raised when the removal pass would flag an implausible share of the
/// library missing. Surfaced (not swallowed) so the worker logs it and the
/// existing index is left intact — the underlying files are almost certainly
/// still on disk behind a transient fault.
#[derive(Debug, thiserror::Error)]
#[error(
    "reindex aborted: {removed} of {total} books ({percent:.0}%) would be flagged missing — \
     refusing to erode the library on a likely-transient scan fault (issue #819)"
)]
pub struct MassMissingError {
    pub removed: usize,
    pub total: usize,
    pub percent: f64,
}

/// Decide whether a scan's enumeration can be trusted to drive the removal
/// pass. Untrustworthy when the walk was `incomplete` (a subdir read
/// failed), or when the root read **totally empty** (`saw_any_file ==
/// false`) while the DB still holds file-backed books under it — the
/// boot-race / unmounted-NFS case where an empty read would otherwise mark
/// the whole library missing.
///
/// `saw_any_file` counts files of *any* extension, so a shared root that
/// legitimately holds files of another format but none of this library's
/// (e.g. EPUBs under an audiobook reindex) stays trustworthy — only a truly
/// empty directory trips the guard.
///
/// A genuinely empty library (`db_has_files == false`) is trustworthy: the
/// first-ever scan of an empty root, or a library the user really did clear,
/// must still be indexable.
fn enumeration_is_trustworthy(incomplete: bool, saw_any_file: bool, db_has_files: bool) -> bool {
    if incomplete {
        return false;
    }
    // A previously-populated root that now reads totally empty is the
    // transient fault, not a real mass-delete — distrust it. Equivalently:
    // trust when the walk saw a file, or when the DB has nothing to lose.
    saw_any_file || !db_has_files
}

/// Trip the circuit-breaker when `removed` exceeds both the absolute floor
/// and [`MASS_MISSING_FRACTION`] of the file-backed library. Returns the
/// typed error to surface; `Ok(())` when the removal is within bounds.
fn check_mass_missing(removed: usize, db_file_backed: usize) -> Result<(), MassMissingError> {
    if removed <= MASS_MISSING_MIN_ABSOLUTE || db_file_backed == 0 {
        return Ok(());
    }
    let fraction = removed as f64 / db_file_backed as f64;
    if fraction > MASS_MISSING_FRACTION {
        return Err(MassMissingError {
            removed,
            total: db_file_backed,
            percent: fraction * 100.0,
        });
    }
    Ok(())
}

/// `true` when a scan's ghost count clears [`MASS_MISSING_WARN_FRACTION`]:
/// more than [`MASS_MISSING_MIN_ABSOLUTE`] books *and* more than the warn
/// fraction of the file-backed library. Reuses the abort guard's absolute
/// floor so an ordinary small-library edit never warns either. Callers only
/// reach this after [`check_mass_missing`] has already passed, so the warn
/// band is implicitly capped above by [`MASS_MISSING_FRACTION`].
fn ghost_warning_threshold_exceeded(removed: usize, db_file_backed: usize) -> bool {
    if removed <= MASS_MISSING_MIN_ABSOLUTE || db_file_backed == 0 {
        return false;
    }
    removed as f64 / db_file_backed as f64 > MASS_MISSING_WARN_FRACTION
}

/// Per-scan tallies handed back to the worker so it can decide whether to
/// attach a [`omnibus_shared::GhostFilesWarning`] to the task's `Done`
/// state. `removed` is this scan's ghost count; `file_backed_total` is the
/// file-backed library size [`check_mass_missing`] measured it against.
#[derive(Debug, Clone, Copy, Default)]
pub struct ReindexStats {
    pub removed: usize,
    pub file_backed_total: usize,
}

impl ReindexStats {
    /// Project this scan's tallies into a wire-facing warning when the
    /// ghost count clears [`MASS_MISSING_WARN_FRACTION`]; `None` below the
    /// threshold, which is the ordinary silent-ghosting path (AC2).
    pub fn ghost_warning(&self) -> Option<omnibus_shared::GhostFilesWarning> {
        if !ghost_warning_threshold_exceeded(self.removed, self.file_backed_total) {
            return None;
        }
        Some(omnibus_shared::GhostFilesWarning {
            removed: u32::try_from(self.removed).unwrap_or(u32::MAX),
            total: u32::try_from(self.file_backed_total).unwrap_or(u32::MAX),
        })
    }
}

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
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        // Clock unreadable (or post-Y292B overflow): substitute `last` so
        // `now - last == 0` and we serve stale (see the doc comment above).
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
    /// A file-backed book whose file is gone (by `books.uuid`). The writer
    /// drops its `book_files` row + clears FTS, retaining the `books` row as
    /// a fileless book; cover files get cleaned up post-commit.
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
///
/// `enumeration_trustworthy` gates the **Removed** bucket only: when the
/// caller could not fully enumerate the tree (a subdir read failed, or a
/// once-populated root read empty) it passes `false`, and `diff_library`
/// leaves `removed` empty so a partial scan can never flag a book missing.
/// The New / Changed / Backfill buckets are unaffected — an incomplete
/// scan still safely indexes the files it *did* see.
///
/// # Bucket semantics
///
/// - **Unchanged** — on disk, in DB, `(mtime_epoch, size_bytes)` matches.
///   No work. `books.id` preserved.
/// - **New** — on disk, not in DB. Full Phase-B parse + insert.
/// - **Changed** — on disk, in DB, stat differs. Full Phase-B parse, then
///   UPDATE in place (preserves `books.id`).
/// - **Removed** — a file-backed book whose file is gone. Its `book_files`
///   row is dropped so the book becomes a **fileless book** (the `books`
///   row + its soft-ref user data are retained, not deleted); a returning
///   file re-attaches via Changed. An already-fileless book is left alone.
///   Only populated when `enumeration_trustworthy` is `true`.
/// - **Backfill** — in DB, in disk, DB has the migration default
///   `(mtime_epoch=0, size_bytes=0)`. Treated as the sentinel for "fs
///   metadata never observed" (post-migration), so the writer only
///   updates the stat columns; the OPF is not re-parsed. Without this,
///   the first reindex after the migration would treat every existing
///   row as Changed.
pub fn diff_library(
    disk: &[ebook::StatEntry],
    db: &[books::IndexedRow],
    library_root: &Path,
    enumeration_trustworthy: bool,
) -> ReindexDiff {
    use std::collections::HashMap;

    // The diff matches on `scan_key` — the library-relative path (F2) — so a
    // scan-root repoint (which leaves relative paths unchanged) preserves
    // every `books.uuid`. Empty scan_keys are synthetic "unreadable subdir"
    // placeholders / pre-backfill rows; skip them on both sides.
    let disk_by_key: HashMap<&str, &ebook::StatEntry> = disk
        .iter()
        .filter(|e| !e.scan_key.is_empty())
        .map(|e| (e.scan_key.as_str(), e))
        .collect();
    let db_by_key: HashMap<&str, &books::IndexedRow> = db
        .iter()
        .filter(|r| !r.scan_key.is_empty())
        .map(|r| (r.scan_key.as_str(), r))
        .collect();

    let mut out = ReindexDiff::default();

    let target = |entry: &ebook::StatEntry| ebook::ParseTarget {
        filename: entry.filename.clone(),
        absolute: library_root.join(&entry.filename),
        mtime_epoch: entry.mtime_epoch,
        size_bytes: entry.size_bytes,
    };

    for (scan_key, entry) in &disk_by_key {
        match db_by_key.get(scan_key) {
            None => out.new.push(target(entry)),
            Some(row) if !row.has_file => {
                // A fileless book whose file is back on disk. Route through
                // Changed: the writer re-creates the `book_files` row while
                // preserving the existing `books.uuid` (auto-relink, F2).
                out.changed.push(target(entry));
            }
            Some(row) => {
                let never_observed = row.mtime_epoch == 0 && row.size_bytes == 0;
                let matches =
                    row.mtime_epoch == entry.mtime_epoch && row.size_bytes == entry.size_bytes;
                if never_observed {
                    out.backfill
                        .push((row.uuid.clone(), entry.mtime_epoch, entry.size_bytes));
                } else if matches {
                    out.unchanged.push(row.uuid.clone());
                } else {
                    out.changed.push(target(entry));
                }
            }
        }
    }

    // The removal pass runs only on a fully-trusted enumeration. On a
    // partial scan (unreadable subdir, or a once-populated root now
    // reading empty) we leave `removed` empty so no book is flagged
    // missing and no `merged_uuids` row is eroded (issue #819).
    if enumeration_trustworthy {
        for row in db {
            if row.scan_key.is_empty() {
                continue;
            }
            if !disk_by_key.contains_key(row.scan_key.as_str()) {
                // A file-backed book whose file is gone becomes a fileless
                // book (the row + its soft-ref user data are retained, not
                // deleted). An already-fileless book is left untouched.
                if row.has_file {
                    out.removed.push(row.uuid.clone());
                }
            }
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
pub async fn reindex(pool: &SqlitePool, library_path: &str) -> anyhow::Result<ReindexStats> {
    reindex_with_progress(pool, library_path, |_, _| {}).await
}

/// [`reindex`] variant that calls `on_progress(processed, total)` after
/// each per-book write inside `sync_books`. Used by
/// [`crate::worker::Worker`] to report determinate `processed / total`
/// counts to the UI indicator. Returns the scan's ghost-count tallies
/// (issue #1057) so the caller can decide whether to attach a warning.
pub async fn reindex_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<ReindexStats> {
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
    let mut db_rows =
        books::list_indexed_rows_for_formats(pool, library_path, ebook::EBOOK_FORMATS).await?;
    // Files attached to a book elsewhere (auto-attach or manual merge)
    // have no books.uuid of their own; their merged_uuids entries stand
    // in here so they classify Unchanged/Changed instead of New.
    db_rows.extend(
        books::list_merged_rows_for_formats(pool, library_path, ebook::EBOOK_FORMATS).await?,
    );
    let library_root: PathBuf = PathBuf::from(library_path);
    let db_file_backed = db_rows.iter().filter(|r| r.has_file).count();
    let trustworthy =
        enumeration_is_trustworthy(stat.incomplete, stat.saw_any_file, db_file_backed > 0);
    if !trustworthy {
        tracing::warn!(
            library_path,
            incomplete = stat.incomplete,
            saw_any_file = stat.saw_any_file,
            db_file_backed,
            "reindex: enumeration incomplete or a populated root read empty — \
             skipping the removal pass; no books marked missing (issue #819)"
        );
    }
    let mut diff = diff_library(&stat.entries, &db_rows, &library_root, trustworthy);
    let removed_count = diff.removed.len();
    check_mass_missing(removed_count, db_file_backed)?;

    // Parse Phase B only for the buckets that need it. `diff.removed`/
    // `.backfill` are read again below, but `.new`/`.changed` are not, so
    // move them out instead of cloning every scan target on every reindex.
    let new_targets = std::mem::take(&mut diff.new);
    let changed_targets = std::mem::take(&mut diff.changed);
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
    sync::sync_books_with_progress(pool, library_path, plan, on_progress).await?;
    gc_missing_files_best_effort(pool).await;
    Ok(ReindexStats {
        removed: removed_count,
        file_backed_total: db_file_backed,
    })
}

/// Best-effort GC of books whose files have been missing past the retention
/// window and that carry no user data. Logged, never surfaced as an error so a
/// GC failure can never abort a reindex — matches the best-effort cover/FTS
/// pattern. The sweep is global (not library-scoped), so running it after each
/// per-library reindex is cheap and idempotent.
async fn gc_missing_files_best_effort(pool: &SqlitePool) {
    match crate::missing_files::gc_books_missing_files(
        pool,
        crate::missing_files::MISSING_FILES_RETENTION_DAYS,
    )
    .await
    {
        Ok(purged) if purged > 0 => tracing::info!(purged, "reindex: GC'd books missing files"),
        Ok(_) => {}
        Err(e) => tracing::error!("reindex: missing-files GC failed: {e}"),
    }
}

/// Audiobook-library sibling of [`reindex`]. Groups audio files by folder,
/// reads multi-part tags, then calls [`sync::sync_audiobooks`] to write
/// `book_file_parts` rows.
pub async fn reindex_audiobooks(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<ReindexStats> {
    reindex_audiobooks_with_progress(pool, library_path, |_, _| {}).await
}

/// Enumeration-trust signals lifted out of Phase A so the caller can gate
/// the removal pass without re-reading the filesystem.
struct EnumerationSignals {
    /// A subdir `read_dir` failed — partial view.
    incomplete: bool,
    /// The walk saw at least one regular file (of any extension).
    saw_any_file: bool,
}

/// Phase A of [`reindex_audiobooks_with_progress`]: stat every audio file
/// under `library_path`, then group the per-file entries into one
/// [`audiobook::AudiobookGroup`] per book (folder-based grouping). The
/// [`EnumerationSignals`] ride alongside so the caller can suppress the
/// removal pass on a partial or empty scan.
async fn stat_and_group_audiobooks(
    library_path: &str,
) -> anyhow::Result<(Vec<audiobook::AudiobookGroup>, EnumerationSignals)> {
    let path_for_scan = library_path.to_owned();
    let library_key = library_path.to_owned();
    let stat = tokio::task::spawn_blocking(move || {
        audiobook::stat_audiobook_library(Some(&path_for_scan), &library_key)
    })
    .await?;
    if let Some(msg) = stat.error {
        anyhow::bail!("audiobook scan of {library_path} failed: {msg}");
    }

    let signals = EnumerationSignals {
        incomplete: stat.incomplete,
        saw_any_file: stat.saw_any_file,
    };
    let entries = stat.entries;
    let library_key2 = library_path.to_owned();
    let groups =
        tokio::task::spawn_blocking(move || audiobook::group_into_books(entries, &library_key2))
            .await?;
    Ok((groups, signals))
}

/// Project audiobook groups to the ebook [`ebook::StatEntry`] shape so
/// `diff_library` can be reused verbatim across both library kinds. Skips
/// attachment-only groups (empty `scan_key`).
fn project_groups_to_stat(groups: &[audiobook::AudiobookGroup]) -> Vec<ebook::StatEntry> {
    groups
        .iter()
        .filter(|g| !g.scan_key.is_empty())
        .map(|g| ebook::StatEntry {
            filename: g.group_path.clone(),
            scan_key: g.scan_key.clone(),
            mtime_epoch: g.max_mtime_epoch,
            size_bytes: g.total_size_bytes,
            error: None,
        })
        .collect()
}

/// [`reindex_audiobooks`] variant that calls `on_progress(processed,
/// total)` after each per-book write inside `sync_audiobooks`. Used by
/// [`crate::worker::Worker`] to report determinate `processed / total`
/// counts to the UI indicator. Returns the scan's ghost-count tallies
/// (issue #1057) so the caller can decide whether to attach a warning.
pub async fn reindex_audiobooks_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<ReindexStats> {
    let (groups, signals) = stat_and_group_audiobooks(library_path).await?;

    // Diff groups against DB rows (project groups to the ebook StatEntry shape
    // so diff_library can be reused verbatim). Scope to audiobook formats so a
    // shared ebook + audiobook library_path does not classify EPUB rows here
    // as Removed (#328).
    let mut db_rows =
        books::list_indexed_rows_for_formats(pool, library_path, audiobook::AUDIOBOOK_FORMATS)
            .await?;
    // Merged/attached audiobook files diff against their book_files stat
    // via merged_uuids — same as the ebook path above.
    db_rows.extend(
        books::list_merged_rows_for_formats(pool, library_path, audiobook::AUDIOBOOK_FORMATS)
            .await?,
    );
    let library_root: PathBuf = PathBuf::from(library_path);
    let groups_as_stat = project_groups_to_stat(&groups);
    let db_file_backed = db_rows.iter().filter(|r| r.has_file).count();
    let trustworthy =
        enumeration_is_trustworthy(signals.incomplete, signals.saw_any_file, db_file_backed > 0);
    if !trustworthy {
        tracing::warn!(
            library_path,
            incomplete = signals.incomplete,
            saw_any_file = signals.saw_any_file,
            db_file_backed,
            "reindex_audiobooks: enumeration incomplete or a populated root read empty — \
             skipping the removal pass; no books marked missing (issue #819)"
        );
    }
    let diff = diff_library(&groups_as_stat, &db_rows, &library_root, trustworthy);
    let removed_count = diff.removed.len();
    check_mass_missing(removed_count, db_file_backed)?;

    // Phase B: parse only the New and Changed groups.
    let groups_by_group_path: std::collections::HashMap<String, audiobook::AudiobookGroup> = groups
        .into_iter()
        .filter(|g| !g.scan_key.is_empty())
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
    sync::sync_audiobooks_with_progress(pool, library_path, plan, on_progress).await?;
    gc_missing_files_best_effort(pool).await;
    Ok(ReindexStats {
        removed: removed_count,
        file_backed_total: db_file_backed,
    })
}

/// Fill `file_chapters` for audiobook `book_files` rows that have none.
///
/// The chapter extraction pipeline was added after the initial audiobook
/// indexer, so books indexed before the migration have zero `file_chapters`
/// rows. The normal diff-based reindex skips unchanged files, so this
/// backfill runs as a separate worker task and is a no-op once all books
/// have chapters. `on_progress(processed, total)` is called after each
/// book so the UI can render a progress bar.
///
/// ## Query efficiency
///
/// All `book_file_parts` rows for the backfill set are fetched in a single
/// `WHERE book_file_id IN (…)` bulk query before the loop rather than one
/// per book, and all chapter inserts are committed in batches of 250 books
/// to avoid per-book WAL flushes (mirrors the sync/audiobooks.rs backfill
/// pattern).
/// Query the first-part filename (ordinal=0) and format for every book
/// under `library_path` that needs chapter backfill (no `file_chapters`
/// rows yet).
async fn fetch_backfill_candidates(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<Vec<(i64, String, String)>> {
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT bf.id, bfp.filename, bf.format \
         FROM book_files bf \
         JOIN books b ON bf.book_id = b.id \
         JOIN scan_roots l ON b.library_id = l.id \
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
    Ok(rows)
}

/// Bulk-fetch all `book_file_parts` rows for `book_file_ids` and group them
/// by `book_file_id` — avoids N per-book SELECT round-trips. Chunked at 500
/// to stay well under SQLite's 999 bind-parameter limit.
async fn bulk_fetch_parts(
    pool: &SqlitePool,
    book_file_ids: &[i64],
) -> anyhow::Result<std::collections::HashMap<i64, Vec<crate::audiobook::AudiobookPart>>> {
    let mut all_parts_rows: Vec<(i64, i64, String, i64, i64, f64)> = Vec::new();
    for chunk in book_file_ids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let parts_sql = format!(
            "SELECT book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds \
             FROM book_file_parts WHERE book_file_id IN ({placeholders}) \
             ORDER BY book_file_id, ordinal"
        );
        let mut parts_query = sqlx::query_as::<_, (i64, i64, String, i64, i64, f64)>(&parts_sql);
        for id in chunk {
            parts_query = parts_query.bind(id);
        }
        all_parts_rows.extend(parts_query.fetch_all(pool).await?);
    }

    let mut parts_by_id: std::collections::HashMap<i64, Vec<crate::audiobook::AudiobookPart>> =
        std::collections::HashMap::new();
    for (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) in
        all_parts_rows
    {
        parts_by_id
            .entry(book_file_id)
            .or_default()
            .push(crate::audiobook::AudiobookPart {
                ordinal,
                filename,
                size_bytes,
                mtime_epoch,
                duration_seconds,
            });
    }
    Ok(parts_by_id)
}

pub(crate) async fn backfill_chapters(
    pool: &SqlitePool,
    library_path: &str,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let rows = fetch_backfill_candidates(pool, library_path).await?;
    if rows.is_empty() {
        return Ok(());
    }

    let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
    tracing::info!(
        count = total,
        "backfilling chapters for existing audiobooks"
    );

    let book_file_ids: Vec<i64> = rows.iter().map(|(id, _, _)| *id).collect();
    let parts_by_id = bulk_fetch_parts(pool, &book_file_ids).await?;

    // Process books in batches of 250 to bound transaction size (mirrors the
    // sync/audiobooks.rs backfill pattern).
    let lib_root = PathBuf::from(library_path);
    for (batch_idx, chunk) in rows.chunks(250).enumerate() {
        let mut tx = pool.begin().await?;
        for (i, (book_file_id, first_part_filename, format)) in chunk.iter().enumerate() {
            let abs = lib_root.join(first_part_filename);
            let fmt = format.clone();
            let chapters =
                tokio::task::spawn_blocking(move || audiobook::extract_chapters(&abs, &fmt))
                    .await
                    .unwrap_or_else(|join_err| {
                        tracing::warn!(
                            book_file_id,
                            %join_err,
                            is_panic = join_err.is_panic(),
                            "chapter extraction task failed; using synthetic fallback"
                        );
                        Vec::new()
                    });

            let parts = parts_by_id
                .get(book_file_id)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            sync::insert_chapters(&mut tx, *book_file_id, &chapters, parts).await?;

            let global_idx = batch_idx * 250 + i;
            let progress = u32::try_from(global_idx)
                .unwrap_or(u32::MAX)
                .saturating_add(1);
            on_progress(progress, total);
        }
        tx.commit().await?;
    }

    Ok(())
}

#[cfg(test)]
mod tests;
