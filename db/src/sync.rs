//! Indexer write path. `sync_books` applies a per-bucket diff atomically,
//! then reconciles the covers directory post-commit; `sync_audiobooks` is
//! the multi-file sibling. Split across the [`fts`], [`books`],
//! [`audiobooks`], [`attach`], [`authors`], and [`backfill`] sub-modules
//! (each self-documented), re-exported here as `omnibus_db::sync::*`.

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
pub use books::{replace_books, sync_books, sync_books_with_progress, MovedFile, SyncPlan};

// The single `books_fts` door. `upsert_fts` / `delete_fts` are
// `pub(crate)` for the in-tx write sites (sync / merge / undo /
// metadata_overrides); `rebuild_all_fts` is `pub` for the worker task +
// admin endpoint that repair the whole index.
pub use fts::rebuild_all_fts;
pub(crate) use fts::{delete_fts, upsert_fts};

/// Push a post-commit cover triple onto `covers`, allocating only when the book actually has a cover.
pub(crate) fn push_cover(
    covers: &mut Vec<(String, String, Vec<u8>)>,
    uuid: &str,
    cover: &Option<(String, Vec<u8>)>,
) {
    if let Some((mime, bytes)) = cover {
        covers.push((uuid.to_string(), mime.clone(), bytes.clone()));
    }
}
