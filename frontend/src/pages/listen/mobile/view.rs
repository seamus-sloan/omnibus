//! Pure view-derivation + time/chapter arithmetic for the mobile player.
//!
//! Everything here is renderer- and browser-free so it can be unit-tested:
//! `H:MM:SS` / `M:SS` formatting, manifest → [`PlayerView`] derivation,
//! chapter-from-position math, previous-chapter seek targets, part
//! auto-advance selection, the playback-rate cycle, and the tokened part
//! URL builder consumed by the audio control surface.

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

/// Wall-clock time left to play `remaining_seconds` of audio content at the
/// given playback `rate` — e.g. it takes half as long to play at 2x speed,
/// even though the content-time remaining is unchanged. Falls back to the
/// raw remaining time for a non-positive rate.
pub fn time_left_at_rate(remaining_seconds: f64, rate: f64) -> f64 {
    if rate > 0.0 {
        remaining_seconds / rate
    } else {
        remaining_seconds
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
/// when `idx` is the last part (playback should stop, not wrap). The runtime
/// auto-advance runs in the JS `ended` handler ([`super::interop`]); this
/// mirrors that decision in Rust so the boundary math is unit-tested.
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
mod tests {
    use super::*;

    fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    #[test]
    fn format_hms_under_one_hour_renders_mm_ss() {
        assert_eq!(format_hms(0.0), "0:00");
        assert_eq!(format_hms(5.0), "0:05");
        assert_eq!(format_hms(65.0), "1:05");
        assert_eq!(format_hms(599.9), "9:59");
    }

    #[test]
    fn format_hms_past_one_hour_renders_h_mm_ss() {
        assert_eq!(format_hms(3600.0), "1:00:00");
        assert_eq!(format_hms(3661.0), "1:01:01");
        assert_eq!(format_hms(13_596.0), "3:46:36");
    }

    #[test]
    fn format_hms_handles_negative_and_non_finite_as_zero() {
        assert_eq!(format_hms(-12.0), "0:00");
        assert_eq!(format_hms(f64::NAN), "0:00");
        assert_eq!(format_hms(f64::INFINITY), "0:00");
    }

    #[test]
    fn format_ms_stays_in_minutes_past_an_hour() {
        assert_eq!(format_ms(0.0), "0:00");
        assert_eq!(format_ms(90.0), "1:30");
        assert_eq!(format_ms(3661.0), "61:01");
        assert_eq!(format_ms(-5.0), "0:00");
    }

    #[test]
    fn format_hm_renders_hours_and_minutes() {
        assert_eq!(format_hm(47.0 * 60.0), "47m");
        assert_eq!(format_hm(13.0 * 3600.0 + 52.0 * 60.0), "13h 52m");
    }

    #[test]
    fn chapter_index_for_elapsed_returns_zero_for_empty_list() {
        assert_eq!(chapter_index_for_elapsed(&[], 60.0), 0);
    }

    #[test]
    fn chapter_index_for_elapsed_tracks_boundaries() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_index_for_elapsed(&chs, 0.0), 0);
        assert_eq!(chapter_index_for_elapsed(&chs, 150.0), 0);
        assert_eq!(chapter_index_for_elapsed(&chs, 300.0), 1);
        assert_eq!(chapter_index_for_elapsed(&chs, 900.0), 1);
    }

    #[test]
    fn remaining_in_chapter_counts_down_to_zero() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert!((remaining_in_chapter(&chs, 0, 100.0) - 200.0).abs() < f64::EPSILON);
        assert!((remaining_in_chapter(&chs, 1, 300.0) - 600.0).abs() < f64::EPSILON);
        // Past the end clamps to zero, and OOB is zero.
        assert_eq!(remaining_in_chapter(&chs, 0, 400.0), 0.0);
        assert_eq!(remaining_in_chapter(&chs, 9, 0.0), 0.0);
    }

    #[test]
    fn time_left_at_rate_scales_inversely_with_speed() {
        assert!((time_left_at_rate(600.0, 1.0) - 600.0).abs() < f64::EPSILON);
        assert!((time_left_at_rate(600.0, 1.5) - 400.0).abs() < f64::EPSILON);
        assert!((time_left_at_rate(600.0, 0.5) - 1200.0).abs() < f64::EPSILON);
    }

    #[test]
    fn time_left_at_rate_falls_back_to_raw_for_non_positive_rate() {
        assert_eq!(time_left_at_rate(600.0, 0.0), 600.0);
        assert_eq!(time_left_at_rate(600.0, -1.0), 600.0);
    }

    #[test]
    fn chapter_prev_seek_none_when_empty_or_oob() {
        assert_eq!(chapter_prev_seek(&[], 10.0, 0), None);
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        assert_eq!(chapter_prev_seek(&chs, 1.0, 5), None);
    }

    #[test]
    fn chapter_prev_seek_restarts_current_when_well_into_it() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_prev_seek(&chs, 350.0, 1), Some(300.0));
    }

    #[test]
    fn chapter_prev_seek_goes_back_when_near_start() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        assert_eq!(chapter_prev_seek(&chs, 301.0, 1), Some(0.0));
        assert_eq!(chapter_prev_seek(&chs, 1.0, 0), Some(0.0));
    }

    #[test]
    fn next_part_index_advances_then_stops_at_end() {
        assert_eq!(next_part_index(3, 0), Some(1));
        assert_eq!(next_part_index(3, 1), Some(2));
        assert_eq!(next_part_index(3, 2), None);
        assert_eq!(next_part_index(1, 0), None);
    }

    #[test]
    fn part_token_url_appends_token_with_correct_separator() {
        assert_eq!(
            part_token_url("http://h:3000", "/api/audiobooks/x/parts/0", Some("tok")),
            "http://h:3000/api/audiobooks/x/parts/0?token=tok"
        );
        // Existing query → `&token=`.
        assert_eq!(
            part_token_url(
                "http://h:3000",
                "/api/audiobooks/x/parts/0?file_id=5",
                Some("t")
            ),
            "http://h:3000/api/audiobooks/x/parts/0?file_id=5&token=t"
        );
        // No token → bare origin-prefixed URL.
        assert_eq!(
            part_token_url("http://h:3000", "/api/audiobooks/x/parts/0", None),
            "http://h:3000/api/audiobooks/x/parts/0"
        );
    }

    #[test]
    fn player_view_from_direct_derives_display_fields() {
        let mut book = EbookMetadata {
            title: Some("A Sea of Glass and Fire".into()),
            filename: "book.m4b".into(),
            accent: Some("#c74".into()),
            ..Default::default()
        };
        book.creators.push(omnibus_shared::Contributor {
            name: "Jane Doe".into(),
            ..Default::default()
        });
        let chs = vec![ch(1, "One", 0.0, 60.0)];
        let v = PlayerView::from_direct(&book, chs, 13.0 * 3600.0 + 52.0 * 60.0, Vec::new());
        assert_eq!(v.title, "A Sea of Glass and Fire");
        assert_eq!(v.author, "Jane Doe");
        assert_eq!(v.accent.as_deref(), Some("#c74"));
        assert_eq!(v.total_label, "13h 52m");
        assert_eq!(v.chapters.len(), 1);
    }

    #[test]
    fn player_view_falls_back_to_filename_and_unknown_author() {
        let book = EbookMetadata {
            title: Some("   ".into()),
            filename: "raw.m4b".into(),
            ..Default::default()
        };
        let v = PlayerView::from_hls(&book);
        assert_eq!(v.title, "raw.m4b");
        assert_eq!(v.author, "Unknown Author");
        assert!(v.chapters.is_empty());
        assert!(v.parts.is_empty());
    }
}
