//! Progress-event plumbing for the indexer's verbose reindex callbacks:
//! the [`ScanUpdate`] wire shape, its phase-label constants, the per-phase
//! `ScanUpdate` builders, and the display-path / tally helpers that
//! [`super::reindex_with_progress`] and its audiobook sibling report
//! through.

use std::path::Path;

use super::ReindexDiff;

/// Phase label reported while the scanner walks the library tree.
pub const PHASE_WALKING: &str = "Walking the library";
/// Phase label reported while Phase B parses new/changed files.
pub const PHASE_PARSING: &str = "Reading file metadata";
/// Phase label reported while the sync writer applies the diff.
pub const PHASE_SYNCING: &str = "Updating the library";

/// One verbose progress event from a reindex: the counted state plus the
/// phase / current-item / tally detail the worker forwards to the UI via
/// [`omnibus_shared::TaskDetail`]. `current_item` inside `detail` is always
/// the library directory name plus the library-relative path — never an
/// absolute server path (the worker status feed is readable by any
/// authenticated user).
#[derive(Debug, Clone)]
pub struct ScanUpdate {
    pub processed: u32,
    pub total: Option<u32>,
    pub detail: omnibus_shared::TaskDetail,
}

impl ScanUpdate {
    /// A count-free event carrying only a phase label — the shape the
    /// pre-count walk phase reports.
    pub(crate) fn phase(phase: &str) -> Self {
        ScanUpdate {
            processed: 0,
            total: None,
            detail: omnibus_shared::TaskDetail {
                phase: Some(phase.to_string()),
                current_item: None,
                tallies: None,
            },
        }
    }
}

/// The display prefix for scan `current_item` paths: the library root's
/// directory name (`/mnt/media/books` → `books`), so the UI shows
/// `books/Author/Title.epub` without leaking where the library lives.
pub(crate) fn root_display_name(library_path: &str) -> String {
    Path::new(library_path)
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Join the root display name with a library-relative path for the UI.
pub(crate) fn display_item(root_name: &str, relative: &str) -> String {
    if root_name.is_empty() {
        relative.to_string()
    } else {
        format!("{root_name}/{relative}")
    }
}

/// Build one Phase-B parse-progress [`ScanUpdate`]: the running `processed`
/// count against the fixed `total`, tagged with the display item and the
/// diff's tallies. Shared by the ebook and audiobook reindex pipelines —
/// their parse phases differ only in what `filename` names.
pub(crate) fn report_parse_progress(
    processed: u32,
    total: u32,
    root_name: &str,
    filename: &str,
    tallies: omnibus_shared::ScanTallies,
) -> ScanUpdate {
    ScanUpdate {
        processed,
        total: Some(total),
        detail: omnibus_shared::TaskDetail {
            phase: Some(PHASE_PARSING.to_string()),
            current_item: Some(display_item(root_name, filename)),
            tallies: Some(tallies),
        },
    }
}

/// Build one sync-phase [`ScanUpdate`]. Shared by the ebook and audiobook
/// reindex pipelines; `current` is `None` for a sync tick that touched no
/// nameable item (e.g. the GC-adjacent bookkeeping ticks).
pub(crate) fn report_sync_progress(
    processed: u32,
    total: u32,
    root_name: &str,
    current: Option<&str>,
    tallies: omnibus_shared::ScanTallies,
) -> ScanUpdate {
    ScanUpdate {
        processed,
        total: Some(total),
        detail: omnibus_shared::TaskDetail {
            phase: Some(PHASE_SYNCING.to_string()),
            current_item: current.map(|c| display_item(root_name, c)),
            tallies: Some(tallies),
        },
    }
}

/// Project a [`ReindexDiff`] (plus the walk's file count) into the wire
/// tallies the UI renders. Saturating: a library past `u32::MAX` files has
/// bigger problems than a pinned tally.
pub(crate) fn diff_tallies(found: usize, diff: &ReindexDiff) -> omnibus_shared::ScanTallies {
    let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
    omnibus_shared::ScanTallies {
        found: count(found),
        new: count(diff.new.len()),
        changed: count(diff.changed.len()),
        removed: count(diff.removed.len()),
        moved: count(diff.moved.len()),
        unchanged: count(diff.unchanged.len()),
    }
}
