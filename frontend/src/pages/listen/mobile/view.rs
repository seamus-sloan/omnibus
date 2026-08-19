//! Pure view-derivation plus time and chapter arithmetic for the mobile player.
//! Everything here is renderer- and browser-free so it can be unit-tested:
//! `H:MM:SS` / `M:SS` formatting, manifest to [`PlayerView`] derivation,
//! chapter-from-position math, previous-chapter seek targets, part auto-advance
//! selection, the playback-rate cycle, and the tokened part URL builder.

use omnibus_shared::{ChapterInfo, EbookMetadata, ManifestPart};

/// The derived, render-ready shape the mobile player draws from. Holds the
/// book metadata (for the cover), display strings, and the chapter map.
#[derive(Clone, PartialEq)]
pub struct PlayerView {
    pub book: EbookMetadata,
    pub title: String,
    pub author: String,
    pub accent: Option<String>,
    pub chapters: Vec<ChapterInfo>,
    pub total_duration: f64,
    /// Human total like `13h 52m` (or `47m`) for the chapters-sheet header.
    pub total_label: String,
    pub parts: Vec<ManifestPart>,
}

impl PlayerView {
    /// Build the view for a direct-play manifest.
    pub fn from_direct(
        book: &EbookMetadata,
        chapters: Vec<ChapterInfo>,
        total_duration: f64,
        parts: Vec<ManifestPart>,
    ) -> Self {
        Self {
            title: title_of(book),
            author: author_of(book),
            accent: book.accent.clone(),
            total_label: format_hm(total_duration),
            chapters,
            total_duration,
            parts,
            book: book.clone(),
        }
    }

    /// Build the (playback-less) view for an HLS manifest — used to render
    /// the cover + title behind the "unsupported format" message.
    pub fn from_hls(book: &EbookMetadata) -> Self {
        Self {
            title: title_of(book),
            author: author_of(book),
            accent: book.accent.clone(),
            total_label: String::new(),
            chapters: Vec::new(),
            total_duration: 0.0,
            parts: Vec::new(),
            book: book.clone(),
        }
    }
}

/// Book title, falling back to the filename.
fn title_of(book: &EbookMetadata) -> String {
    match book.title.as_deref() {
        Some(t) if !t.trim().is_empty() => t.to_string(),
        _ => book.filename.clone(),
    }
}

/// First creator's name, or `Unknown Author`.
fn author_of(book: &EbookMetadata) -> String {
    book.creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string())
}

/// Format `seconds` as `H:MM:SS` (or `M:SS` under an hour). Non-finite /
/// negative inputs render as `0:00`.
pub fn format_hms(seconds: f64) -> String {
    let s_total = bounded_secs(seconds);
    let h = s_total / 3600;
    let m = (s_total % 3600) / 60;
    let s = s_total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Format `seconds` as `M:SS` — always minutes:seconds, even past an hour
/// (so a 90-minute chapter reads `90:00`). Used for within-chapter readouts.
pub fn format_ms(seconds: f64) -> String {
    let s_total = bounded_secs(seconds);
    let m = s_total / 60;
    let s = s_total % 60;
    format!("{m}:{s:02}")
}

/// Coarse `Nh Mm` / `Mm` label for a whole-book total.
fn format_hm(seconds: f64) -> String {
    let s_total = bounded_secs(seconds);
    let h = s_total / 3600;
    let m = (s_total % 3600) / 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// Clamp `seconds` to a whole, in-range `u64` (guarding NaN/inf/negative
/// and the `as u64` truncation).
fn bounded_secs(seconds: f64) -> u64 {
    if !seconds.is_finite() || seconds < 0.0 {
        return 0;
    }
    let bounded = seconds.min(f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let v = bounded as u64;
    v
}

/// Derive the current chapter index from `elapsed` and a chapter list sorted
/// by `start_seconds`. Returns 0 when `chapters` is empty.
pub fn chapter_index_for_elapsed(chapters: &[ChapterInfo], elapsed: f64) -> usize {
    if chapters.is_empty() {
        return 0;
    }
    chapters
        .partition_point(|c| c.start_seconds <= elapsed)
        .saturating_sub(1)
}

/// Seconds remaining within the chapter at `idx` given the whole-book
/// `elapsed`. Returns 0 when out of bounds.
pub fn remaining_in_chapter(chapters: &[ChapterInfo], idx: usize, elapsed: f64) -> f64 {
    match chapters.get(idx) {
        Some(c) => ((c.start_seconds + c.duration_seconds) - elapsed).max(0.0),
        None => 0.0,
    }
}

/// Resolve the "previous chapter" seek target:
/// - >3s into the current chapter → its start;
/// - within 3s of the start and not the first → the previous chapter's start;
/// - otherwise → 0.
///
/// `None` when `chapters` is empty or `idx` is out of bounds.
pub fn chapter_prev_seek(chapters: &[ChapterInfo], elapsed: f64, idx: usize) -> Option<f64> {
    let current = chapters.get(idx)?;
    let target = if elapsed - current.start_seconds > 3.0 {
        current.start_seconds
    } else if let Some(prev) = idx.checked_sub(1).and_then(|i| chapters.get(i)) {
        prev.start_seconds
    } else {
        0.0
    };
    Some(target)
}

/// Pick the next playable part after `idx` given a part count. Returns `None`
/// when `idx` is the last part (playback should stop, not wrap).
///
/// Intentionally kept unwired: the runtime decision runs synchronously inside
/// the JS `ended` handler ([`super::interop`]) so the next part's `src` can be
/// set with zero playback gap — routing through a Rust/WASM round-trip
/// (`dioxus.send` + a response) would add audible latency at every part
/// boundary. This mirrors that same `idx + 1 < part_count` check so the
/// boundary arithmetic stays unit-tested in Rust.
#[allow(dead_code)]
pub fn next_part_index(part_count: usize, idx: usize) -> Option<usize> {
    let next = idx + 1;
    (next < part_count).then_some(next)
}

/// Build the authenticated URL for a manifest part on mobile: prefix the
/// server origin and append the session token as `?token=` (or `&token=`
/// when the part URL already carries a query, e.g. `?file_id=`). An `<audio
/// src>` fetch can't carry a bearer header, so the token must ride the query.
pub fn part_token_url(server_url: &str, part_path: &str, token: Option<&str>) -> String {
    let base = format!("{server_url}{part_path}");
    match token {
        Some(t) => {
            let sep = if part_path.contains('?') { '&' } else { '?' };
            format!("{base}{sep}token={t}")
        }
        None => base,
    }
}

#[cfg(test)]
mod tests;
