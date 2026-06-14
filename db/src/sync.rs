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
//! * [`fts`] — the single `books_fts` choke-point (`upsert_fts` /
//!   `delete_fts` / `rebuild_all_fts`). Every book mutation routes its
//!   FTS maintenance through here so the standalone index can't drift.
//! * [`books`] — the transactional orchestrator (`sync_books`), the
//!   per-bucket helpers (`sync_removed`, `sync_changed`, `sync_new`,
//!   `stamp_last_indexed`), `replace_books`, the per-book row writers
//!   (`insert_book_row`, `update_book_row`), and the metadata-link
//!   dispatch + cover side helpers (FTS is delegated to [`fts`]).
//! * [`audiobooks`] — `sync_audiobooks` and its `AudiobookSyncPlan`
//!   payload, plus the audiobook-specific row / parts / FTS inserts.
//! * [`attach`] — cross-format auto-attach: lookups + writers that hang
//!   a newly indexed file off an existing book in another format and
//!   record it in `merged_uuids` for reindex protection.
//! * [`authors`] — the batched author-link writer (`insert_author_links`).
//! * [`backfill`] — the stat-only `(uuid, mtime_epoch, size_bytes)`
//!   chunked UPDATE used to fill in the post-migration sentinels.
//!
//! Public API is re-exported here so callers (`server/`, `frontend/`,
//! sibling `db/` modules) keep importing through `omnibus_db::sync::*`
//! unchanged.

mod attach;
mod audiobooks;
mod authors;
mod backfill;
mod books;
mod fts;

#[cfg(test)]
mod tests;

pub(crate) use audiobooks::insert_chapters;
pub use audiobooks::{sync_audiobooks, sync_audiobooks_with_progress, AudiobookSyncPlan};
pub use books::{replace_books, sync_books, sync_books_with_progress, SyncError, SyncPlan};

// The single `books_fts` door. `upsert_fts` / `delete_fts` are
// `pub(crate)` for the in-tx write sites (sync / merge / undo /
// metadata_overrides); `rebuild_all_fts` is `pub` for the worker task +
// admin endpoint that repair the whole index.
pub use fts::rebuild_all_fts;
pub(crate) use fts::{delete_fts, upsert_fts};
