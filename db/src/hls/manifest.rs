//! HLS VOD manifest builder + ffmpeg `-progress pipe:1` parsing helpers.
//! Pure functions, no I/O — exercised directly by the unit tests so the
//! transcode runner stays a thin orchestrator on top.

use super::HlsPart;

/// Build an HLS VOD manifest from the stored part durations.
///
/// Each segment is 10 seconds; the last segment gets the remainder.
/// When all parts have `duration_seconds = 0` (not yet lofty-probed) a
/// minimal single-segment stub is returned so the frontend can still load the
/// resource before the first real transcode finishes.
pub fn build_manifest(parts: &[HlsPart]) -> String {
    const TARGET: f64 = 10.0;
    // Cap: 100 hours of audio at 10s/segment. Real audiobooks max out
    // well below this; anything past it is corrupt tag data and we'd
    // rather serve the minimal stub than allocate ~100MB of `#EXTINF:`
    // text from a NaN-derived loop count.
    const MAX_SEGMENTS: f64 = 36_000.0;
    // Minimal stub so hls.js/Safari can discover the URL is valid before
    // the duration probe / transcode finishes. Also the safety fallback
    // for non-finite / out-of-range totals.
    const MIN_STUB: &str = concat!(
        "#EXTM3U\n",
        "#EXT-X-VERSION:3\n",
        "#EXT-X-TARGETDURATION:10\n",
        "#EXT-X-PLAYLIST-TYPE:VOD\n",
        "#EXT-X-MEDIA-SEQUENCE:0\n",
        "#EXTINF:0.001,\n",
        "seg-0000.ts\n",
        "#EXT-X-ENDLIST\n"
    );

    let total_secs: f64 = parts.iter().map(|p| p.duration_seconds).sum();

    // `<= 0.0` doesn't catch NaN — guard both, otherwise a corrupt tag's
    // NaN propagates through the division below and emits `#EXTINF:NaN`.
    if !total_secs.is_finite() || total_secs <= 0.0 {
        return MIN_STUB.to_string();
    }

    let segments_f = (total_secs / TARGET).ceil();
    if segments_f > MAX_SEGMENTS {
        return MIN_STUB.to_string();
    }
    let segments_f = segments_f.max(1.0);
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let num_segments = segments_f as usize;
    let mut m3u8 = String::with_capacity(num_segments * 40);
    m3u8.push_str("#EXTM3U\n");
    m3u8.push_str("#EXT-X-VERSION:3\n");
    m3u8.push_str("#EXT-X-TARGETDURATION:10\n");
    m3u8.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    m3u8.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");

    for i in 0..num_segments {
        let dur = if i == num_segments - 1 {
            // `num_segments` is clamped above to a value well within the
            // f64-exactly-representable integer range, so the `i as f64`
            // cast cannot lose precision here.
            #[allow(clippy::cast_precision_loss)]
            let i_f = i as f64;
            total_secs - i_f * TARGET
        } else {
            TARGET
        };
        m3u8.push_str(&format!("#EXTINF:{dur:.3},\nseg-{i:04}.ts\n"));
    }
    m3u8.push_str("#EXT-X-ENDLIST\n");
    m3u8
}

/// Parse one ffmpeg `-progress pipe:1` line and return the encode timeline
/// position in microseconds when the line is the `out_time_us=<int>` (or
/// `out_time_ms=<int>`, ffmpeg confusingly uses microseconds for both)
/// progress key. Returns `None` for every other key — `frame=`, `bitrate=`,
/// `progress=continue`, etc. Public for unit testing.
pub fn parse_ffmpeg_progress_us(line: &str) -> Option<u64> {
    let line = line.trim();
    let (key, value) = line.split_once('=')?;
    // ffmpeg has historically emitted both `out_time_us=` (named correctly
    // since ~5.0) and `out_time_ms=` (a long-standing typo that actually
    // carries microseconds). Accept both so the parser keeps working across
    // versions.
    if key != "out_time_us" && key != "out_time_ms" {
        return None;
    }
    let value = value.trim();
    // `N/A` shows up during the initial warmup before ffmpeg has produced
    // any output frames. Drop it cleanly rather than treating it as 0.
    if value == "N/A" {
        return None;
    }
    value.parse::<u64>().ok()
}

/// Compute the heartbeat fraction to write to `.progress` from an
/// `out_time_us` value and the total duration in seconds. Clamped to
/// `[0.01, 0.95]` so the orphan detector always sees motion and the
/// success sentinel `1.0` is reserved for the finalize step.
/// Public for unit testing.
pub fn ffmpeg_progress_fraction(out_time_us: u64, total_secs: f64) -> f32 {
    // `<= 0.0` doesn't catch NaN — guard both so a corrupt-tag NaN
    // total can't propagate through the division and leak NaN into
    // the `.progress` sentinel.
    if !total_secs.is_finite() || total_secs <= 0.0 {
        return 0.05;
    }
    // `out_time_us` is microseconds since transcode start; even at the
    // f64-exact ceiling (~2^53 µs ≈ 285 years) precision is well beyond
    // anything ffmpeg would report.
    #[allow(clippy::cast_precision_loss)]
    let elapsed_secs = (out_time_us as f64) / 1_000_000.0;
    let frac = (elapsed_secs / total_secs).clamp(0.01, 0.95);
    #[allow(clippy::cast_possible_truncation)]
    let f32_frac = frac as f32;
    f32_frac
}
