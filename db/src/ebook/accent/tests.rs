//! Unit tests for `ebook::accent` — exercises the cover-accent extractor
//! across the happy path (saturated PNG → clamped OKLCH string), the
//! decoder-rejection paths (empty, corrupt, oversized bytes), the
//! achromatic short-circuits (pure black, pure gray), and a perf-budget
//! guard that catches multi-second regressions on realistic cover sizes.

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
