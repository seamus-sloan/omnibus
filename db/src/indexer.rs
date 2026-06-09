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

impl From<crate::settings::SettingsError> for IndexerError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => IndexerError::Db(inner),
        }
    }
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
        let fmt = format.clone();
        let chapters = tokio::task::spawn_blocking(move || audiobook::extract_chapters(&abs, &fmt))
            .await
            .unwrap_or_default();

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
mod tests;
