//! Manual format merge: absorb one `books` row (the **source**) into
//! another (the **target**) so a work that indexed as two format-siblings
//! becomes one book with multiple `book_files` entries. `merge_books`
//! runs the whole move in one transaction and records a JSON snapshot of
//! the source in `merge_log` for [`undo_merge`].
//!
//! Concurrency: uses a plain deferred `pool.begin()` like the sync
//! writers. SQLite's single-writer lock serializes the merge against a
//! concurrent reindex; the residual race — a diff snapshot taken before
//! the merge commits — self-heals because the sync New bucket consults
//! `merged_uuids` before inserting.

mod snapshot;
mod transaction;
mod undo;

#[cfg(test)]
mod tests;

pub use transaction::merge_books;
pub use undo::undo_merge;

/// Predictable failure space for the merge/undo surface. The UI renders
/// a per-variant message, so the variants stay caller-branchable.
#[derive(Debug, thiserror::Error)]
pub enum MergeError {
    #[error("source and target are the same book")]
    SameBook,
    #[error("book not found: {0}")]
    BookNotFound(String),
    #[error("both books already have a {0} file")]
    FormatCollision(String),
    #[error("merge log entry not found")]
    LogNotFound,
    #[error("merge was already undone")]
    AlreadyUndone,
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("merge snapshot encode/decode failed: {0}")]
    Snapshot(#[from] serde_json::Error),
}

/// What [`merge_books`] hands back to the caller: the audit-log id (the
/// undo handle) and the surviving book's uuid (for the redirect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub merge_log_id: i64,
    pub target_uuid: String,
}
