//! Manual format merge for the admin merge RPC: absorb one `books`
//! row (the **source**) into another (the **target**) so a work
//! indexed as two format-siblings becomes one book with multiple
//! `book_files`. `merge_books` runs in one transaction and snapshots
//! the source into `merge_log` for [`undo_merge`].

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
