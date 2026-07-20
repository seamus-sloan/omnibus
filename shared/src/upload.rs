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

/// Tag-derived metadata returned by the audiobook inspect step — the audiobook sibling of [`UploadInspection`].
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct AudiobookInspection {
    pub title: Option<String>,
    pub author: Option<String>,
    /// Whether any uploaded part carried embedded cover art.
    pub has_cover: bool,
    /// Lowercased format the server settled on (`"m4b"`, `"m4a"`, `"mp3"`).
    pub format: String,
    /// Number of files in the upload (1 for a single `.m4b`/`.m4a`; N for a
    /// multi-part `.mp3` audiobook filed into one folder).
    pub part_count: usize,
    /// Combined runtime across every part, when tags supplied durations.
    pub duration_seconds: Option<f64>,
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

/// Number of leading bytes [`detect_audiobook_format`] needs to classify an
/// upload. An MP4 `ftyp` box lives at offset 4 and its type field runs to
/// offset 8, so 8 bytes is enough; MP3 is decided from the first 3.
pub const AUDIOBOOK_MAGIC_LEN: usize = 8;

/// Detect an uploadable audiobook container family from magic bytes, returning
/// the representative lowercase family (`"mp4"` for `.m4a`/`.m4b`, `"mp3"` for
/// `.mp3`). The caller keeps the uploaded `.m4a`/`.m4b` distinction from the
/// filename — both share one ISO-BMFF container, so bytes alone can't tell them
/// apart. Like [`detect_ebook_format`] this is a cheap first gate; a successful
/// `lofty` parse in the inspect handler is the second.
pub fn detect_audiobook_format(bytes: &[u8]) -> Option<&'static str> {
    // ISO Base Media File Format (MP4): a `ftyp` box type at offset 4. Covers
    // `.m4a` and `.m4b` regardless of the specific brand (`M4A `, `M4B `,
    // `isom`, `mp42`, …).
    if bytes.len() >= AUDIOBOOK_MAGIC_LEN && &bytes[4..8] == b"ftyp" {
        return Some("mp4");
    }
    // MP3: an ID3v2 tag header, or a raw MPEG-audio frame sync (11 set bits).
    if bytes.starts_with(b"ID3") {
        return Some("mp3");
    }
    if bytes.len() >= 2 && bytes[0] == 0xFF && (bytes[1] & 0xE0) == 0xE0 {
        return Some("mp3");
    }
    None
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

    #[test]
    fn detect_audiobook_format_accepts_mp4_ftyp_box() {
        // 4-byte box size, then `ftyp`, then a brand — an m4a/m4b opening.
        let m4b = b"\x00\x00\x00\x1cftypM4B ";
        assert_eq!(detect_audiobook_format(m4b), Some("mp4"));
    }

    #[test]
    fn detect_audiobook_format_accepts_id3_and_frame_sync_mp3() {
        assert_eq!(
            detect_audiobook_format(b"ID3\x04\x00\x00\x00\x00"),
            Some("mp3")
        );
        // MPEG-1 Layer III frame sync (0xFFFB).
        assert_eq!(
            detect_audiobook_format(&[0xFF, 0xFB, 0x90, 0x00]),
            Some("mp3")
        );
    }

    #[test]
    fn detect_audiobook_format_rejects_non_audio() {
        assert_eq!(detect_audiobook_format(b"PK\x03\x04\x14\x00\x00\x00"), None);
        assert_eq!(detect_audiobook_format(b"%PDF-1.7"), None);
        assert_eq!(detect_audiobook_format(b"ID"), None);
    }
}
