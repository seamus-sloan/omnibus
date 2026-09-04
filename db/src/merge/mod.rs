//! Manual format merge for the admin merge RPC: absorb one `books`
//! row (the **source**) into another (the **target**) so a work
//! indexed as two format-siblings becomes one book with multiple
//! `book_files`. `merge_books` runs in one transaction and snapshots
//! into `merge_log` for [`undo_merge`] — the source book, and for the
//! per-reader state whose collision the merge resolves destructively,
//! *both* books (see `curation`).

mod curation;
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
    #[error("merge log entry not found")]
    LogNotFound,
    #[error("merge was already undone")]
    AlreadyUndone,
    /// Undo would have to choose between a reader's pre-merge curation and a
    /// change they made to the survivor afterwards. Both are real, so undo
    /// refuses and says which field and which reader to look at.
    #[error("cannot undo this merge: {0}")]
    UndoConflict(String),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("merge snapshot encode/decode failed: {0}")]
    Snapshot(#[from] serde_json::Error),
    #[error(transparent)]
    Physical(#[from] crate::physical::PhysicalError),
    /// A non-database failure surfaced from a dependency of the merge/undo
    /// path. Coarse and message-carrying: the UI treats it as an opaque
    /// internal failure, so no caller branches on the source.
    #[error("{0}")]
    Other(String),
}

/// What [`merge_books`] hands back to the caller: the audit-log id (the
/// undo handle) and the surviving book's uuid (for the redirect).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub merge_log_id: i64,
    pub target_uuid: String,
}
