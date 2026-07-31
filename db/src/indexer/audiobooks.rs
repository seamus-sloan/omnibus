//! Audiobook-library reindex pipeline: the [`reindex_audiobooks`] sibling of
//! [`super::reindex`], grouping audio files by folder and diffing them
//! through the shared [`super::diff_library`] classifier.

use std::path::PathBuf;

use sqlx::SqlitePool;

use super::{
    check_mass_missing, diff_library, enumeration_is_trustworthy, gc_missing_files_best_effort,
    ReindexStats,
};
use crate::{audiobook, books, sync};

/// Audiobook-library sibling of [`super::reindex`]. Groups audio files by
/// folder, reads multi-part tags, then calls [`sync::sync_audiobooks`] to
/// write `book_file_parts` rows.
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
    let moved_count = diff.moved.len();
    check_mass_missing(removed_count, db_file_backed)?;
    if moved_count > 0 {
        tracing::info!(
            library_path,
            moved = moved_count,
            "reindex_audiobooks: matched relocated groups by stat pair"
        );
    }

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
        moved: diff.moved,
        removed_uuids: diff.removed,
        backfill: diff.backfill,
    };
    sync::sync_audiobooks_with_progress(pool, library_path, plan, on_progress).await?;
    gc_missing_files_best_effort(pool).await;
    Ok(ReindexStats {
        removed: removed_count,
        file_backed_total: db_file_backed,
        moved: moved_count,
    })
}
