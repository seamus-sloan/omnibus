//! Embedded cover artwork extraction for audiobooks. Walks the primary
//! tag's pictures and returns the first one with non-empty data, mapped
//! to a `(mime, bytes)` pair the existing cover pipeline already accepts.

use std::path::Path;

use lofty::file::TaggedFileExt;
use lofty::picture::MimeType;

use super::parse::AudiobookError;

/// Best-effort embedded-cover read. `None` when the file has no embedded
/// artwork; the indexer treats that as "no cover, render the typographic
/// plate" — same fallback ebooks without embedded covers take.
pub(super) fn extract_cover(path: &Path) -> Result<Option<(String, Vec<u8>)>, AudiobookError> {
    let tagged = lofty::read_from_path(path)?;
    let tag = match tagged.primary_tag().or_else(|| tagged.first_tag()) {
        Some(t) => t,
        None => return Ok(None),
    };
    for pic in tag.pictures() {
        let data = pic.data();
        if data.is_empty() {
            continue;
        }
        let mime = mime_to_str(pic.mime_type());
        return Ok(Some((mime.to_string(), data.to_vec())));
    }
    Ok(None)
}

fn mime_to_str(mime: Option<&MimeType>) -> &'static str {
    match mime {
        Some(MimeType::Png) => "image/png",
        Some(MimeType::Jpeg) => "image/jpeg",
        Some(MimeType::Tiff) => "image/tiff",
        Some(MimeType::Bmp) => "image/bmp",
        Some(MimeType::Gif) => "image/gif",
        // Unknown/missing mime defaults to JPEG — overwhelmingly the
        // shape of embedded audiobook art, and matches the EPUB-cover
        // fallback in `crate::ebook::cover`.
        _ => "image/jpeg",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Absolute path to a committed audiobook fixture under repo-root
    /// `test_data/audiobooks/`. `CARGO_MANIFEST_DIR` is the `db/` crate dir,
    /// so `..` climbs to the workspace root where the fixtures live.
    fn fixture(rel: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test_data")
            .join("audiobooks")
            .join(rel)
    }

    #[test]
    fn extract_cover_returns_embedded_png_bytes_when_file_has_artwork() {
        // The generated fixture carries an embedded `image/png` APIC frame.
        let path = fixture("generated/ada_lovelace_solo/the_analytical_audiobook.mp3");
        let (mime, bytes) = extract_cover(&path)
            .expect("read should succeed")
            .expect("fixture has embedded artwork");
        assert_eq!(mime, "image/png");
        assert!(!bytes.is_empty());
        // Confirm the bytes are the actual PNG payload, not a stub.
        // `starts_with` can't panic on a short payload the way slicing would.
        assert!(bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]));
    }

    #[test]
    fn extract_cover_returns_none_when_valid_audio_has_no_embedded_picture() {
        // A real, tag-bearing MP3 with no APIC frame — the common case for
        // library files that ship without cover art.
        let path = fixture(
            "public_domain/James Whitcomb Riley/A Song of Long Ago/01_a_song_of_long_ago.mp3",
        );
        let cover = extract_cover(&path).expect("read should succeed");
        assert!(cover.is_none());
    }

    #[test]
    fn extract_cover_errors_when_path_does_not_exist() {
        // A missing file can't be probed by lofty, so the read surfaces as
        // an `Err` (not `Ok(None)`); the caller in `parse.rs` logs a WARN
        // and proceeds with no cover. lofty maps the missing file to either
        // a bare `Io` or a `LoftyError`-wrapped `Tag` depending on platform,
        // so accept either error variant rather than pinning one.
        let dir = tempfile::tempdir().expect("should create tempdir");
        let missing = dir.path().join("omnibus_cover_test_missing.mp3");
        let err = extract_cover(&missing).expect_err("missing path should error");
        assert!(
            matches!(err, AudiobookError::Io(_) | AudiobookError::Tag(_)),
            "got {err:?}"
        );
    }
}
