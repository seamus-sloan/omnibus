//! Wire types for the admin book-merge surface (`/api/rpc/merge-books`
//! and the REST mirror `/api/books/merge`).

use serde::{Deserialize, Serialize};

/// Result of a successful merge: the audit-log id (the undo handle) and
/// the surviving book's uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeBooksResult {
    pub merge_log_id: i64,
    pub target_uuid: String,
}

/// Request body for the REST merge endpoint: merge `source_uuid` into
/// `target_uuid` (the target survives).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeBooksRequest {
    pub source_uuid: String,
    pub target_uuid: String,
}

/// Request body for the REST unmerge endpoint, naming the `merge_log`
/// entry to reverse — the undo handle a merge response carried.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoMergeRequest {
    pub merge_log_id: i64,
}

/// Result of a successful unmerge: the restored (source) book's uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndoMergeResult {
    pub restored_uuid: String,
}
