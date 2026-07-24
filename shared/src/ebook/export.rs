//! Wire shape for the OPF-export action result (F5.8).

use serde::{Deserialize, Serialize};

/// Result of exporting a book's metadata to its `metadata.opf` sidecar.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpfExportResult {
    /// Absolute path of the written `metadata.opf`.
    pub path: String,
    /// Whether an existing `metadata.opf` was backed up to
    /// `metadata.opf.bak` before writing.
    pub backed_up: bool,
}
