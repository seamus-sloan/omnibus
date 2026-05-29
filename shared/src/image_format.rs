//! Magic-byte image format detection shared by the REST upload handler
//! (`server::backend`) and the server-function bodies (`frontend::rpc`).
//!
//! Lives here in `omnibus-shared` — which both `server` and `frontend`
//! already depend on — so the sniff has a single source of truth. It is
//! pure byte inspection (no `image`-crate dependency), so it stays free of
//! transport/platform deps and compiles on every target.

/// Detect image format from magic bytes. Returns the canonical MIME type for
/// accepted raster formats (JPEG, PNG, GIF, WebP). Returns `None` for
/// unrecognised or dangerous formats (SVG, arbitrary binaries).
pub fn detect_image_format(bytes: &[u8]) -> Option<String> {
    if bytes.len() < 4 {
        return None;
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg".into())
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47]) {
        Some("image/png".into())
    } else if bytes.starts_with(b"GIF8") {
        Some("image/gif".into())
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp".into())
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_jpeg() {
        assert_eq!(
            detect_image_format(&[0xFF, 0xD8, 0xFF, 0xE0]),
            Some("image/jpeg".into())
        );
    }

    #[test]
    fn detects_png() {
        let png = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        assert_eq!(detect_image_format(&png), Some("image/png".into()));
    }

    #[test]
    fn detects_gif() {
        assert_eq!(detect_image_format(b"GIF89a..."), Some("image/gif".into()));
    }

    #[test]
    fn detects_webp() {
        // "RIFF" + 4-byte length + "WEBP".
        let webp = b"RIFF\x00\x00\x00\x00WEBPVP8 ";
        assert_eq!(detect_image_format(webp), Some("image/webp".into()));
    }

    #[test]
    fn rejects_too_short_input() {
        assert_eq!(detect_image_format(&[0xFF, 0xD8]), None);
    }

    #[test]
    fn rejects_unrecognised_bytes() {
        // SVG / arbitrary text must not be accepted.
        assert_eq!(detect_image_format(b"<svg xmlns=..."), None);
        assert_eq!(detect_image_format(&[0x00, 0x01, 0x02, 0x03]), None);
    }

    #[test]
    fn rejects_riff_without_webp_marker() {
        // A RIFF container that isn't WebP (e.g. WAV/AVI) is not an image.
        let riff_wav = b"RIFF\x00\x00\x00\x00WAVEfmt ";
        assert_eq!(detect_image_format(riff_wav), None);
    }
}
