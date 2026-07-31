//! Re-encode a user's override cover to match the format of the EPUB's
//! existing cover resource. The in-place rewrite swaps only the
//! *bytes* of the cover entry, so the replacement must decode to the same
//! image format the manifest already declares — otherwise the entry's
//! `media-type` and href extension would lie about its contents.

use std::io::Cursor;

use image::ImageFormat;

/// Produce cover bytes encoded to `target_mime` from an override image.
///
/// Returns the override bytes unchanged when it is already the target format
/// (the common case — most covers and most uploads are JPEG). Otherwise
/// decodes and re-encodes. Returns `None` (never an error) when the override
/// can't be decoded or the target format can't be encoded — the caller then
/// leaves the original cover in place rather than failing the whole export.
pub(super) fn encode_cover_for(
    target_mime: &str,
    override_mime: &str,
    override_bytes: Vec<u8>,
) -> Option<Vec<u8>> {
    let target = format_for_mime(target_mime)?;
    if format_for_mime(override_mime) == Some(target) {
        return Some(override_bytes);
    }

    let img = match image::load_from_memory(&override_bytes) {
        Ok(img) => img,
        Err(e) => {
            tracing::warn!(target: "omnibus::epub_rewrite", %e, "override cover undecodable; keeping original cover");
            return None;
        }
    };

    let mut out = Cursor::new(Vec::new());
    // JPEG has no alpha channel; flatten to RGB8 so an RGBA source encodes.
    let result = if target == ImageFormat::Jpeg {
        image::DynamicImage::ImageRgb8(img.to_rgb8()).write_to(&mut out, target)
    } else {
        img.write_to(&mut out, target)
    };
    match result {
        Ok(()) => Some(out.into_inner()),
        Err(e) => {
            tracing::warn!(target: "omnibus::epub_rewrite", %e, target = target_mime, "override cover re-encode failed; keeping original cover");
            None
        }
    }
}

/// Map a cover `media-type` to an `image` codec, or `None` for a format the
/// enabled `image` features can't round-trip.
fn format_for_mime(mime: &str) -> Option<ImageFormat> {
    match mime.trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => Some(ImageFormat::Jpeg),
        "image/png" => Some(ImageFormat::Png),
        "image/gif" => Some(ImageFormat::Gif),
        "image/webp" => Some(ImageFormat::WebP),
        _ => None,
    }
}
