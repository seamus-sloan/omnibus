//! Audiobook-library reindex pipeline: the [`reindex_audiobooks`] sibling of
//! [`super::reindex`], grouping audio files by folder and diffing them
//! through the shared [`super::diff_library`] classifier.

use std::path::PathBuf;

use sqlx::SqlitePool;

use crate::{audiobook, books, sync};

use super::{
    check_mass_missing, diff_library, diff_tallies, enumeration_is_trustworthy,
    gc_missing_files_best_effort, promote_authorless_unchanged, report_parse_progress,
    report_sync_progress, root_display_name, ReindexDiff, ReindexOptions, ReindexStats, ScanUpdate,
    PHASE_WALKING,
};

/// Audiobook-library sibling of [`super::reindex`]. Groups audio files by
/// folder, reads multi-part tags, then calls [`sync::sync_audiobooks`] to
/// write `book_file_parts` rows.
pub async fn reindex_audiobooks(
    pool: &SqlitePool,
    library_path: &str,
) -> anyhow::Result<ReindexStats> {
    reindex_audiobooks_with_progress(pool, library_path, |_| {}).await
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

/// Project audiobook groups to the ebook [`crate::ebook::StatEntry`] shape so
/// `diff_library` can be reused verbatim across both library kinds. Skips
/// attachment-only groups (empty `scan_key`).
fn project_groups_to_stat(groups: &[audiobook::AudiobookGroup]) -> Vec<crate::ebook::StatEntry> {
    groups
        .iter()
        .filter(|g| !g.scan_key.is_empty())
        .map(|g| crate::ebook::StatEntry {
            filename: g.group_path.clone(),
            scan_key: g.scan_key.clone(),
            mtime_epoch: g.max_mtime_epoch,
            size_bytes: g.total_size_bytes,
            error: None,
        })
        .collect()
}

/// Phase-A output of [`reindex_audiobooks_with_progress`]: the grouped
/// audiobooks plus the walk's diff, breaker counts, and progress tallies —
/// mirrors the ebook pipeline's own Phase-A aggregate in
/// [`super::reindex_with_progress`].
struct AudiobookDiffPhase {
    groups: Vec<audiobook::AudiobookGroup>,
    diff: ReindexDiff,
    removed_count: usize,
    moved_count: usize,
    db_file_backed: usize,
    tallies: omnibus_shared::ScanTallies,
    root_name: String,
}

/// Phase A of [`reindex_audiobooks_with_progress`]: group the library,
/// diff against the DB, and derive the walk's fixed breaker counts and
/// progress tallies up front so every later parse/sync event reports the
/// same numbers.
async fn diff_audiobook_library_for_reindex(
    pool: &SqlitePool,
    library_path: &str,
    options: ReindexOptions,
) -> anyhow::Result<AudiobookDiffPhase> {
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
    let mut diff = diff_library(&groups_as_stat, &db_rows, &library_root, trustworthy);
    if options.relink_authorless {
        promote_authorless_unchanged(pool, &mut diff, &groups_as_stat, &db_rows, &library_root)
            .await?;
    }
    let removed_count = diff.removed.len();
    let moved_count = diff.moved.len();
    check_mass_missing(removed_count, db_file_backed)?;
    if moved_count > 0 {
        tracing::info!(
            library_path,
            moved = moved_count,
            "reindex_audiobooks: matched relocated groups by stat pair"
        );
    }

    // Tallies are fixed once the diff lands; every parse and sync event
    // carries the same copy so the panel's counts never regress mid-scan.
    let tallies = diff_tallies(groups_as_stat.len(), &diff);
    let root_name = root_display_name(library_path);

    Ok(AudiobookDiffPhase {
        groups,
        diff,
        removed_count,
        moved_count,
        db_file_backed,
        tallies,
        root_name,
    })
}

/// Resolve the diff's New/Changed filename buckets back to full
/// [`audiobook::AudiobookGroup`]s for Phase-B parsing. `groups` is consumed
/// (moved into a lookup map) since Phase A has no further use for it.
fn select_parse_groups(
    groups: Vec<audiobook::AudiobookGroup>,
    diff: &ReindexDiff,
) -> (
    Vec<audiobook::AudiobookGroup>,
    Vec<audiobook::AudiobookGroup>,
) {
    let groups_by_group_path: std::collections::HashMap<String, audiobook::AudiobookGroup> = groups
        .into_iter()
        .filter(|g| !g.scan_key.is_empty())
        .map(|g| (g.group_path.clone(), g))
        .collect();

    let new_groups = diff
        .new
        .iter()
        .filter_map(|t| groups_by_group_path.get(&t.filename).cloned())
        .collect();
    let changed_groups = diff
        .changed
        .iter()
        .filter_map(|t| groups_by_group_path.get(&t.filename).cloned())
        .collect();
    (new_groups, changed_groups)
}

/// [`reindex_audiobooks`] variant that reports verbose [`ScanUpdate`]
/// events, mirroring [`super::reindex_with_progress`]: a [`PHASE_WALKING`]
/// event before the walk, per-group parse events during the tag read, and
/// per-book sync events inside `sync_audiobooks` — parse and sync events
/// carry the diff's [`omnibus_shared::ScanTallies`] and the current
/// group's display path. Returns the scan's ghost-count tallies (issue
/// #1057) so the caller can decide whether to attach a warning.
pub async fn reindex_audiobooks_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    on_progress: impl FnMut(ScanUpdate) + Send + 'static,
) -> anyhow::Result<ReindexStats> {
    reindex_audiobooks_with_options(pool, library_path, ReindexOptions::default(), on_progress)
        .await
}

/// [`reindex_audiobooks_with_progress`] variant taking [`ReindexOptions`] —
/// currently just the authorless relink pass posted by the blocklist
/// recovery flow.
pub async fn reindex_audiobooks_with_options(
    pool: &SqlitePool,
    library_path: &str,
    options: ReindexOptions,
    on_progress: impl FnMut(ScanUpdate) + Send + 'static,
) -> anyhow::Result<ReindexStats> {
    let mut on_progress = on_progress;
    on_progress(ScanUpdate::phase(PHASE_WALKING));

    let phase_a = diff_audiobook_library_for_reindex(pool, library_path, options).await?;

    // Phase B: parse only the New and Changed groups.
    let (new_groups, changed_groups) = select_parse_groups(phase_a.groups, &phase_a.diff);

    let parse_total = u32::try_from(new_groups.len() + changed_groups.len()).unwrap_or(u32::MAX);
    let root_for_parse = PathBuf::from(library_path);
    let root_name_for_parse = phase_a.root_name.clone();
    let tallies = phase_a.tallies;
    // The callback moves into the blocking task for per-group reporting and
    // rides back out in the return value for the sync phase below.
    let (parsed, on_progress) = tokio::task::spawn_blocking(move || {
        let mut parsed_count = 0u32;
        let mut report = |g: &audiobook::AudiobookGroup| {
            parsed_count = parsed_count.saturating_add(1);
            on_progress(report_parse_progress(
                parsed_count,
                parse_total,
                &root_name_for_parse,
                &g.scan_key,
                tallies,
            ));
        };
        let new_books =
            audiobook::parse_groups_with_progress(new_groups, &root_for_parse, &mut report);
        let changed_books =
            audiobook::parse_groups_with_progress(changed_groups, &root_for_parse, &mut report);
        ((new_books, changed_books), on_progress)
    })
    .await?;
    let mut on_progress = on_progress;

    let plan = sync::AudiobookSyncPlan {
        new_books: parsed.0,
        changed_books: parsed.1,
        moved: phase_a.diff.moved,
        removed_uuids: phase_a.diff.removed,
        backfill: phase_a.diff.backfill,
    };
    sync::sync_audiobooks_with_progress(pool, library_path, plan, |processed, total, current| {
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
