//! Resume-point meta formatting shared by the mobile resume card and the web
//! continue-reading hero: percent/remaining labels for audio, and the plain
//! continue affordance for epub rows that lack a stored percent.

use omnibus_shared::{ProgressFormat, ResumePoint};

/// Meta line + progress percentage for a resume point. Audio rows with known
/// totals get "Ch. N · 42% · 7h 50m left"; audio without totals falls back to
/// the raw position; epub rows read as a plain continue affordance.
pub(super) fn resume_meta(point: &ResumePoint) -> (String, Option<i64>) {
    if point.record.format == ProgressFormat::Epub {
        // A stored whole-book percent (a Kobo's write, the comic pager's
        // page/count mapping, or the percent the server derives for a web
        // CFI write) is honest enough for a bar; a bare CFI is not.
        return match point.record.progress_percent {
            Some(pct) => (format!("{pct}% \u{00b7} Continue reading"), Some(pct)),
            None => ("Continue reading".to_string(), None),
        };
    }
    let pos = point.record.audio_position_seconds.unwrap_or(0.0);
    match point.total_duration_seconds.filter(|t| *t > 0.0) {
        Some(total) => {
            // Clamped to 0..=100 above, so the cast is in-range (NaN → 0).
            #[allow(clippy::cast_possible_truncation)]
            let pct = ((pos / total).clamp(0.0, 1.0) * 100.0).round() as i64;
            let left = format_hm_left((total - pos).max(0.0));
            let ch = point
                .chapter_number
                .map(|n| format!("Ch. {n} \u{00b7} "))
                .unwrap_or_default();
            (format!("{ch}{pct}% \u{00b7} {left} left"), Some(pct))
        }
        None => (format!("{} in", format_hms_short(pos)), None),
    }
}

/// `7h 50m` / `50m` label for a remaining-seconds span.
pub(super) fn format_hm_left(seconds: f64) -> String {
    // Durations are finite and non-negative at both call sites; the clamp
    // keeps an absurd input from saturating the cast.
    #[allow(clippy::cast_possible_truncation)]
    let total_min = (seconds / 60.0).round().clamp(0.0, f64::from(i32::MAX)) as i64;
    let h = total_min / 60;
    let m = total_min % 60;
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// `H:MM:SS` (or `M:SS`) for an absolute position.
pub(super) fn format_hms_short(seconds: f64) -> String {
    // The finite/positive guard plus the clamp keep the cast in-range.
    #[allow(clippy::cast_possible_truncation)]
    let s = if seconds.is_finite() && seconds > 0.0 {
        seconds.min(f64::from(i32::MAX)) as i64
    } else {
        0
    };
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_shared::{EbookMetadata, ProgressRecord};

    fn point(format: ProgressFormat, pos: Option<f64>, total: Option<f64>) -> ResumePoint {
        ResumePoint {
            record: ProgressRecord {
                book_uuid: "u".into(),
                format,
                epub_cfi: None,
                audio_position_seconds: pos,
                progress_percent: None,
                kobo_location: None,
                book_file_id: None,
                updated_at: 0,
                client_updated_at: 0,
            },
            book: EbookMetadata::default(),
            linked: false,
            cross_format: None,
            total_duration_seconds: total,
            chapter_number: Some(3),
            chapter_count: Some(10),
        }
    }

    #[test]
    fn resume_meta_reports_percent_and_time_left_for_audio_with_total() {
        let (meta, pct) = resume_meta(&point(ProgressFormat::Audio, Some(3600.0), Some(7200.0)));
        assert_eq!(pct, Some(50));
        assert_eq!(meta, "Ch. 3 \u{00b7} 50% \u{00b7} 1h 00m left");
    }

    #[test]
    fn resume_meta_has_no_bar_for_epub_or_totalless_audio() {
        let (meta, pct) = resume_meta(&point(ProgressFormat::Epub, None, None));
        assert_eq!((meta.as_str(), pct), ("Continue reading", None));

        let (meta, pct) = resume_meta(&point(ProgressFormat::Audio, Some(95.0), None));
        assert_eq!((meta.as_str(), pct), ("1:35 in", None));
    }

    #[test]
    fn resume_meta_shows_a_bar_for_epub_rows_carrying_a_percent() {
        let mut p = point(ProgressFormat::Epub, None, None);
        p.record.progress_percent = Some(37);
        let (meta, pct) = resume_meta(&p);
        assert_eq!(pct, Some(37));
        assert_eq!(meta, "37% \u{00b7} Continue reading");
    }

    #[test]
    fn format_helpers_render_hours_and_minutes() {
        assert_eq!(format_hm_left(4712.0), "1h 19m");
        assert_eq!(format_hm_left(240.0), "4m");
        assert_eq!(format_hms_short(3725.0), "1:02:05");
        assert_eq!(format_hms_short(f64::NAN), "0:00");
    }
}
