//! Cover-accent color extraction.
//!
//! Decodes cover bytes, bins pixels into hue buckets, and returns a single
//! representative `oklch(L C H)` accent string. Called from the parse
//! phase in [`super::parse`] so the indexer can persist it alongside the
//! cover.

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
mod tests {
    use super::*;
    use crate::ebook::test_support::*;

    // ── F1.7 accent extraction ─────────────────────────────────────────

    #[test]
    fn extract_accent_returns_oklch_for_saturated_cover() {
        let bytes = solid_color_png(200, 60, 50, 64, 96);
        let accent = extract_accent(&bytes).expect("saturated cover yields accent");
        assert!(
            accent.starts_with("oklch("),
            "accent should be an oklch() string, got {accent}"
        );
        // Clamps in extract_accent require the L value to stay readable.
        let mid = accent
            .trim_start_matches("oklch(")
            .trim_end_matches(')')
            .split_whitespace()
            .next()
            .unwrap()
            .parse::<f32>()
            .unwrap();
        assert!(
            (0.55..=0.78).contains(&mid),
            "lightness {mid} should be clamped to [0.55, 0.78]"
        );
    }

    #[test]
    fn extract_accent_returns_none_for_empty_bytes() {
        assert!(extract_accent(&[]).is_none());
    }

    #[test]
    fn extract_accent_returns_none_for_corrupt_bytes() {
        assert!(extract_accent(b"not an image, just text").is_none());
    }

    #[test]
    fn extract_accent_returns_none_for_oversized_bytes() {
        // Start from a *valid* cover that decodes to an accent on its own, so
        // the only thing keeping the oversized case out of the decoder is the
        // size guard. (All-zero bytes would fail to decode regardless, which
        // wouldn't catch a regression that removed the guard.)
        let mut bytes = solid_color_png(200, 60, 50, 64, 96);
        assert!(
            extract_accent(&bytes).is_some(),
            "sanity: the unpadded cover should decode to an accent"
        );
        // Pad past the cap with trailing bytes the PNG decoder ignores (it
        // stops at IEND). Without the guard these would still decode and
        // yield Some, so this assertion fails if the cap check is removed.
        bytes.resize(MAX_EMBEDDED_COVER_BYTES + 1, 0);
        assert!(
            extract_accent(&bytes).is_none(),
            "oversized cover bytes should be rejected by the size guard before decoding"
        );
    }

    #[test]
    fn extract_accent_returns_none_for_pure_black() {
        let bytes = solid_color_png(0, 0, 0, 64, 96);
        assert!(
            extract_accent(&bytes).is_none(),
            "all-black cover should produce no accent"
        );
    }

    #[test]
    fn extract_accent_returns_none_for_pure_gray() {
        let bytes = solid_color_png(128, 128, 128, 64, 96);
        assert!(
            extract_accent(&bytes).is_none(),
            "grayscale cover should produce no accent (no chroma)"
        );
    }

    #[test]
    fn extract_accent_completes_within_budget() {
        // Real EPUB covers top out around 1500×2250. Test against that size
        // with a generous debug-mode budget — release builds run ~3× faster.
        // The point of the test is to catch a regression to seconds, not to
        // hold a tight production-grade budget in unoptimized builds. GitHub
        // Actions ubuntu-latest runners have measurably slower per-core
        // throughput than developer workstations and can spend 600–700 ms on
        // this input in debug builds, so the threshold is set to 2 s — any
        // multi-second regression still trips it.
        let bytes = solid_color_png(80, 140, 200, 1500, 2250);
        let start = std::time::Instant::now();
        let _ = extract_accent(&bytes);
        let elapsed = start.elapsed();
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "extract_accent must stay well under multiple seconds on realistic input; took {elapsed:?}"
        );
    }
}
