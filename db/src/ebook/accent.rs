//! Cover-accent color extraction: decodes cover bytes, bins pixels into hue
//! buckets, and returns one representative `oklch(L C H)` accent string. Called
//! from [`super::parse`] so the indexer can persist it alongside the cover.

/// Hard cap on embedded cover bytes we'll hand to the `image` decoder.
/// Higher than the 10 MiB HTTP cap in `author_photos` because these are
/// trusted local files; 20 MiB still covers uncompressed print-resolution
/// covers while bounding the decode allocation against a crafted EPUB.
const MAX_EMBEDDED_COVER_BYTES: usize = 20 * 1024 * 1024;

/// Extract a representative accent color from cover bytes. Returns an
/// `oklch(L C H)` string clamped to a readable band, or `None` when decoding
/// fails or the cover has no chromatic content. The algorithm hue-buckets
/// the pixels, picks the highest-weighted bucket, and converts to OKLCH —
/// see `docs/design/atrium-design-system.md` §2b for the rationale.
pub fn extract_accent(bytes: &[u8]) -> Option<String> {
    if bytes.is_empty() {
        return None;
    }
    if bytes.len() > MAX_EMBEDDED_COVER_BYTES {
        return None;
    }
    let img = image::load_from_memory(bytes).ok()?;
    let small = img.thumbnail(32, 48).to_rgb8();

    let mut buckets = [AccentBucket::default(); 12];
    for px in small.pixels() {
        let r = px[0] as f32 / 255.0;
        let g = px[1] as f32 / 255.0;
        let b = px[2] as f32 / 255.0;
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        // Filter near-black and near-grayscale pixels so they don't
        // overwhelm the actual artwork color.
        if max < 0.10 || delta < 0.06 {
            continue;
        }
        let hue = if delta == 0.0 {
            0.0
        } else if (max - r).abs() < f32::EPSILON {
            60.0 * (((g - b) / delta).rem_euclid(6.0))
        } else if (max - g).abs() < f32::EPSILON {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        let sat = if max == 0.0 { 0.0 } else { delta / max };
        let weight = sat * max;
        // `hue` is in [0.0, 360.0) from the HSV formula above, so the
        // floored quotient is a small non-negative integer (≤ 12)
        // before `.min(11)` clamps it into the bucket array.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let idx = ((hue / 30.0).floor() as usize).min(11);
        buckets[idx].r += r * weight;
        buckets[idx].g += g * weight;
        buckets[idx].b += b * weight;
        buckets[idx].w += weight;
    }

    let best = buckets
        .iter()
        .max_by(|a, b| a.w.partial_cmp(&b.w).unwrap_or(std::cmp::Ordering::Equal))?;
    if best.w == 0.0 {
        return None;
    }
    let r = best.r / best.w;
    let g = best.g / best.w;
    let b = best.b / best.w;
    let (l, c, h) = rgb_to_oklch(r, g, b);
    let l = l.clamp(0.55, 0.78);
    let c = c.clamp(0.06, 0.18);
    Some(format!("oklch({l:.3} {c:.3} {h:.1})"))
}

#[derive(Default, Clone, Copy)]
struct AccentBucket {
    r: f32,
    g: f32,
    b: f32,
    w: f32,
}

/// Convert non-linear sRGB in [0, 1] to OKLCH. Matrix from Björn Ottosson,
/// <https://bottosson.github.io/posts/oklab/>. Returns `(L, C, H°)`.
fn rgb_to_oklch(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    fn linearize(v: f32) -> f32 {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    }
    // OKLab matrix coefficients (Björn Ottosson). Truncated to f32 precision
    // — clippy's `excessive_precision` lint flags the full 10-digit form,
    // and the extra digits don't survive `f32` round-off anyway.
    let r = linearize(r);
    let g = linearize(g);
    let b = linearize(b);
    let l = 0.412_221_47 * r + 0.536_332_54 * g + 0.051_445_995 * b;
    let m = 0.211_903_5 * r + 0.680_699_5 * g + 0.107_396_96 * b;
    let s = 0.088_302_46 * r + 0.281_718_85 * g + 0.629_978_7 * b;
    let l_ = l.cbrt();
    let m_ = m.cbrt();
    let s_ = s.cbrt();
    let big_l = 0.210_454_26 * l_ + 0.793_617_8 * m_ - 0.004_072_047 * s_;
    let a = 1.977_998_5 * l_ - 2.428_592_2 * m_ + 0.450_593_7 * s_;
    let b_oklab = 0.025_904_037 * l_ + 0.782_771_77 * m_ - 0.808_675_77 * s_;
    let c = (a * a + b_oklab * b_oklab).sqrt();
    let mut h = b_oklab.atan2(a).to_degrees();
    if h < 0.0 {
        h += 360.0;
    }
    (big_l, c, h)
}

#[cfg(test)]
mod tests;
