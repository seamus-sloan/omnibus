//! Wire shape for the OPF-export action result.

use serde::{Deserialize, Serialize};

/// Result of exporting a book's metadata to its `metadata.opf` sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpfExportResult {
    /// Library-relative path of the written `metadata.opf` (the server's
    /// absolute filesystem prefix is never sent to the client — a `can_edit`
    /// user isn't necessarily an admin).
    pub path: String,
    /// Whether an existing `metadata.opf` was backed up to
    /// `metadata.opf.bak` before writing.
    pub backed_up: bool,
}
