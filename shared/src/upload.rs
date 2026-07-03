//! Wire types for the "add your own books" upload flow + magic-byte format
//! detection.
//!
//! The web client uploads a file, the server parses it and returns an
//! [`UploadInspection`] for an editable confirm step, then the client commits
//! and gets back an [`UploadCommitResult`]. Shared here so the REST handler
//! (`server::backend::uploads`) and the frontend data layer agree on the shape.

use serde::{Deserialize, Serialize};

/// Auto-extracted metadata returned by the inspect step. Each field mirrors
/// what the indexer would read from the file's embedded metadata, so the UI
/// can pre-fill the editable confirm form. The user can correct any field
/// before committing; the corrected `title`/`author` drive the on-disk folder.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadInspection {
    pub title: Option<String>,
    pub author: Option<String>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    pub language: Option<String>,
    /// Whether the file carried an embedded (or sidecar) cover.
    pub has_cover: bool,
    /// Lowercased file extension the server settled on (e.g. `"epub"`).
    pub ext: String,
}

/// Result of a successful commit: the durable uuid of the newly-filed book,
/// so the client can navigate straight to its detail page.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UploadCommitResult {
    pub uuid: String,
}

/// Detect an uploadable ebook format from magic bytes. Returns the canonical
/// lowercase extension for accepted formats. Mirrors
/// [`crate::image_format::detect_image_format`] — pure byte inspection, no
/// parser dependency, so it compiles on every target.
///
/// EPUB is a ZIP archive, so it carries the ZIP local-file-header magic
/// `PK\x03\x04`. A successful parse in the inspect handler is the second gate;
/// this sniff just rejects obviously-wrong uploads (text, images, truncated
/// files) before the heavier parse runs.
pub fn detect_ebook_format(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() < 4 {
        return None;
    }
    // ZIP local file header — every non-empty EPUB starts with this.
    if bytes.starts_with(&[0x50, 0x4B, 0x03, 0x04]) {
        Some("epub")
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_ebook_format_accepts_zip_magic() {
        // `PK\x03\x04` then arbitrary trailing bytes — an EPUB's opening bytes.
        let epub = b"PK\x03\x04\x14\x00\x00\x00";
        assert_eq!(detect_ebook_format(epub), Some("epub"));
    }

    #[test]
    fn detect_ebook_format_rejects_non_zip() {
        assert_eq!(detect_ebook_format(b"%PDF-1.7"), None);
        assert_eq!(detect_ebook_format(b"<html>"), None);
        // Empty-archive end-of-central-directory magic is not a real EPUB.
        assert_eq!(detect_ebook_format(b"PK\x05\x06"), None);
    }

    #[test]
    fn detect_ebook_format_rejects_too_short_input() {
        assert_eq!(detect_ebook_format(b"PK"), None);
    }
}
