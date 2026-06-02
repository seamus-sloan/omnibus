//! Indexer write path. `sync_books` applies a per-bucket diff
//! (New / Changed / Removed / Backfill) atomically, then reconciles the
//! covers directory post-commit. `replace_books` is the nuke-and-pave
//! compatibility shim that the test suite drives.
//!
//! `sync_audiobooks` is the multi-file audiobook sibling: same four-bucket
//! diff, but each "book" also writes `book_file_parts` rows (one per source
//! audio file) so the HLS pipeline has an ordered part list.
//!
//! The implementation is split across focused sub-modules:
//!
//! * [`books`] — the transactional orchestrator (`sync_books`), the
//!   per-bucket helpers (`sync_removed`, `sync_changed`, `sync_new`,
//!   `stamp_last_indexed`), `replace_books`, the per-book row writers
//!   (`insert_book_row`, `update_book_row`), and the metadata-link
//!   dispatch + FTS / cover side helpers.
//! * [`audiobooks`] — `sync_audiobooks` and its `AudiobookSyncPlan`
//!   payload, plus the audiobook-specific row / parts / FTS inserts.
//! * [`authors`] — the batched author-link writer (`insert_author_links`).
//! * [`backfill`] — the stat-only `(uuid, mtime_epoch, size_bytes)`
//!   chunked UPDATE used to fill in the post-migration sentinels.
//!
//! Public API is re-exported here so callers (`server/`, `frontend/`,
//! sibling `db/` modules) keep importing through `omnibus_db::sync::*`
//! unchanged.

mod audiobooks;
mod authors;
mod backfill;
mod books;

#[cfg(test)]
mod tests;

pub use audiobooks::{sync_audiobooks, AudiobookSyncPlan};
pub use books::{replace_books, sync_books, SyncError, SyncPlan};

// `pub(crate)` re-export for sibling `db/` modules (currently
// `metadata_overrides`) that referenced this helper at
// `crate::sync::insert_fts_row` before the split.
pub(crate) use books::insert_fts_row;
