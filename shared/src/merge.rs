//! Wire types for the admin book-merge surface (`/api/rpc/merge-books`).

use serde::{Deserialize, Serialize};

/// Result of a successful merge: the audit-log id (the undo handle) and
/// the surviving book's uuid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeBooksResult {
    pub merge_log_id: i64,
    pub target_uuid: String,
}
