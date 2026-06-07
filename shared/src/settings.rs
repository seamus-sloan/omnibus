//! User-configurable library paths and the `GET /api/library` response shape.
//!
//! These types straddle every client (mobile + web) and the server, so they
//! live here rather than next to the handler that produces them.

use serde::{Deserialize, Serialize};

/// User-configurable paths for the ebook and audiobook libraries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub ebook_library_path: Option<String>,
    pub audiobook_library_path: Option<String>,
}

/// One half of the library listing (either ebooks or audiobooks).
///
/// `counts_by_ext` is an ordered list of `(extension, count)` pairs for the
/// extensions the caller asked the scanner to track. Order matches the
/// caller-provided extension list so the UI can render a predictable summary
/// line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySection {
    pub path: Option<String>,
    pub total_files: usize,
    pub counts_by_ext: Vec<(String, usize)>,
    pub error: Option<String>,
}

/// Response payload for `GET /api/library`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryContents {
    pub ebooks: LibrarySection,
    pub audiobooks: LibrarySection,
}
