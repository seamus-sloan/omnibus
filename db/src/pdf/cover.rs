//! Page-1 cover raster via the pure-Rust `hayro` renderer, encoded as JPEG
//! for the covers store. No system dependency: the standard-14 fonts are
//! embedded in the renderer, so an unembedded Helvetica title page still
//! draws.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::page::Page;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, RenderCache, RenderSettings};

use crate::ebook::{resolve_cover_with, ScanOptions};

/// Long edge of the rendered cover, in pixels. Matches the size the thumbs
/// pipeline downsamples from for a scanned cover, and keeps a poster-sized
/// page from rendering at print resolution.
const COVER_LONG_EDGE_PX: f32 = 1200.0;
/// JPEG quality for the stored cover.
const COVER_JPEG_QUALITY: u8 = 85;

/// Render the first page as the cover candidate. `None` when the document
/// has no pages, the page has no area, or the renderer panics on it — a
/// cover is advisory, never worth failing the book over.
pub(super) fn render_first_page(pdf: &Pdf) -> Option<(String, Vec<u8>)> {
    let page = pdf.pages().first()?;
    catch_unwind(AssertUnwindSafe(|| render_page(page))).unwrap_or_else(|_| {
        tracing::warn!("pdf cover render panicked; leaving the book coverless");
        None
    })
}

fn render_page(page: &Page<'_>) -> Option<(String, Vec<u8>)> {
    let (width, height) = page.render_dimensions();
    if width <= 0.0 || height <= 0.0 {
        return None;
    }
    let scale = (COVER_LONG_EDGE_PX / width.max(height)).min(4.0);
    let cache = RenderCache::new();
    let pixmap = render(
        page,
        &cache,
        &InterpreterSettings::default(),
        &RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..Default::default()
        },
    );
    let (w, h) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
    if w == 0 || h == 0 {
        return None;
    }
    // Premultiplied RGBA over an opaque white ground is plain RGB for every
    // pixel, so dropping the alpha channel is the whole conversion.
    let mut rgb = Vec::with_capacity((w * h * 3) as usize);
    for px in pixmap.data_as_u8_slice().chunks_exact(4) {
        rgb.extend_from_slice(&px[..3]);
    }
    let image = image::RgbImage::from_raw(w, h, rgb)?;
    let mut out = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, COVER_JPEG_QUALITY)
        .encode_image(&image)
        .ok()?;
    Some(("image/jpeg".to_string(), out))
}

/// Read-only cover lookup for the cover backfill — the PDF twin of
/// `ebook::extract_cover`: sidecar first, else a fresh page-1 render, never
/// writing a sidecar into the library.
pub fn extract_cover(path: &Path) -> Option<(String, Vec<u8>)> {
    resolve_cover_with(path, &ScanOptions::default(), || {
        let bytes = std::fs::read(path).ok()?;
        let pdf = catch_unwind(AssertUnwindSafe(|| Pdf::new(bytes)))
            .ok()?
            .ok()?;
        render_first_page(&pdf)
    })
}
