//! Ebook-library reindex pipeline: the [`reindex`] / [`reindex_with_progress`]
//! entry points, diffing the walk through the shared [`super::diff_library`]
//! classifier. Mirrors [`super::audiobooks`]'s own reindex pipeline.

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::{books, ebook, sync};

use super::{
    check_mass_missing, diff_library, diff_tallies, enumeration_is_trustworthy,
    gc_missing_files_best_effort, report_parse_progress, report_sync_progress, root_display_name,
    ReindexDiff, ReindexStats, ScanUpdate, PHASE_WALKING,
};

/// Scan `library_path`, diff against the existing index, and apply only
/// the per-book changes the diff demands. Runs the scan on the blocking
/// pool so callers can `await` it from a normal async context without
/// blocking the runtime.
///
/// A fatal scan error (missing or unreadable root) is returned as `Err`
/// and the existing index is **not** touched — we'd rather serve
/// stale-but-good data than wipe the table and mark the index "fresh"
/// (which would also suppress retries until [`super::REFRESH_AFTER_SECS`]
/// elapses). Per-book parse failures are *not* fatal; they land in the
/// DB as rows with `error = Some(_)`, same as before.
pub async fn reindex(pool: &SqlitePool, library_path: &str) -> anyhow::Result<ReindexStats> {
    reindex_with_progress(pool, library_path, |_| {}).await
}

/// Phase-A output of [`reindex_with_progress`]: the ebook diff plus the
/// walk's breaker counts and progress tallies — mirrors the audiobook
/// pipeline's own Phase-A aggregate, `audiobooks::AudiobookDiffPhase`.
struct EbookDiffPhase {
    diff: ReindexDiff,
    removed_count: usize,
    moved_count: usize,
    db_file_backed: usize,
    tallies: omnibus_shared::ScanTallies,
    root_name: String,
}

/// Phase A of [`reindex_with_progress`]: stat the ebook library, diff the
/// walk against the DB, and derive the walk's fixed breaker counts and
/// progress tallies up front so every later parse/sync event reports the
/// same numbers.
async fn diff_ebook_library_for_reindex(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<EbookDiffPhase> {
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
    let diff = diff_library(&stat.entries, &db_rows, &library_root, trustworthy);
    let removed_count = diff.removed.len();
    let moved_count = diff.moved.len();
    // Moved files never entered `removed`, so a reorganization — however
    // large — can't trip the breaker that exists to catch a vanished mount.
    check_mass_missing(removed_count, db_file_backed)?;
    if moved_count > 0 {
        tracing::info!(
            library_path,
            moved = moved_count,
            "reindex: matched relocated files by stat pair"
        );
    }

    // Tallies are fixed once the diff lands; every parse and sync event
    // carries the same copy so the panel's counts never regress mid-scan.
    let found = stat
        .entries
        .iter()
        .filter(|e| !e.scan_key.is_empty())
        .count();
    let tallies = diff_tallies(found, &diff);
    let root_name = root_display_name(library_path);

    Ok(EbookDiffPhase {
        diff,
        removed_count,
        moved_count,
        db_file_backed,
        tallies,
        root_name,
    })
}

/// [`reindex`] variant that reports verbose [`ScanUpdate`] events: a
/// [`PHASE_WALKING`] event before the tree walk, a per-file parse event
/// during the Phase-B metadata parse, and a per-book sync event after each
/// write inside `sync_books` — the parse and sync events carry the diff's
/// [`omnibus_shared::ScanTallies`] and the current item's display path.
/// Used by [`crate::worker::Worker`] to feed the UI's live activity panel.
/// `Send + 'static` because the callback rides into the Phase-B
/// `spawn_blocking` (and back out). Returns the scan's ghost-count tallies
/// so the caller can decide whether to attach a warning.
pub async fn reindex_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    on_progress: impl FnMut(ScanUpdate) + Send + 'static,
) -> anyhow::Result<ReindexStats> {
    let mut on_progress = on_progress;
    on_progress(ScanUpdate::phase(PHASE_WALKING));

    let mut phase_a = diff_ebook_library_for_reindex(pool, library_path).await?;

    // Parse Phase B only for the buckets that need it. `diff.removed`/
    // `.backfill` are read again below, but `.new`/`.changed` are not, so
    // move them out instead of cloning every scan target on every reindex.
    let new_targets = std::mem::take(&mut phase_a.diff.new);
    let changed_targets = std::mem::take(&mut phase_a.diff.changed);
    let parse_total = u32::try_from(new_targets.len() + changed_targets.len()).unwrap_or(u32::MAX);
    let root_for_parse = phase_a.root_name.clone();
    let tallies = phase_a.tallies;
    // The callback moves into the blocking task for per-file reporting and
    // rides back out in the return value for the sync phase below.
    let (parsed, on_progress) = tokio::task::spawn_blocking(move || {
        // Materialize cover sidecars so future scans skip the zip
        // (F0.6). Best-effort: read-only filesystems fall through to the
        // in-memory bytes for the current scan and retry next time.
        let opts = ebook::ScanOptions {
            materialize_sidecars: true,
        };
        let mut parsed_count = 0u32;
        let mut report = |t: &ebook::ParseTarget| {
            parsed_count = parsed_count.saturating_add(1);
            on_progress(report_parse_progress(
                parsed_count,
                parse_total,
                &root_for_parse,
                &t.filename,
                tallies,
            ));
        };
        let new_books =
            ebook::parse_ebook_targets_with_progress(new_targets, opts.clone(), &mut report);
        let changed_books =
            ebook::parse_ebook_targets_with_progress(changed_targets, opts, &mut report);
        ((new_books, changed_books), on_progress)
    })
    .await?;
    let mut on_progress = on_progress;

    let plan = sync::SyncPlan {
        new_books: parsed.0,
        changed_books: parsed.1,
        moved: phase_a.diff.moved,
        removed_uuids: phase_a.diff.removed,
        backfill: phase_a.diff.backfill,
    };
    sync::sync_books_with_progress(pool, library_path, plan, |processed, total, current| {
        on_progress(report_sync_progress(
            processed,
            total,
            &phase_a.root_name,
            current,
            tallies,
        ));
    })
    .await?;
    gc_missing_files_best_effort(pool).await;
    Ok(ReindexStats {
        removed: phase_a.removed_count,
        file_backed_total: phase_a.db_file_backed,
        moved: phase_a.moved_count,
    })
}
