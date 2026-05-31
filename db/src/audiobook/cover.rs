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
