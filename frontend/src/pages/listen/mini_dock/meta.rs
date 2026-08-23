//! Pure display math for the mini-dock: progress percent, the active-state
//! gate shared with the `/read` route, and the subtitle line (chapter ·
//! clock · time-left).

use super::super::helpers::{format_hms, remaining_at_rate};

/// Progress fill percent (0–100) for the dock's mini progress bar. Returns 0
/// when the duration is unknown or `elapsed` is non-finite so the bar never
/// renders a NaN width.
pub(super) fn progress_pct(elapsed: f64, duration: f64) -> f64 {
    if duration <= 0.0 || !elapsed.is_finite() {
        return 0.0;
    }
    ((elapsed / duration) * 100.0).clamp(0.0, 100.0)
}

/// Whether the mini-dock has both a loaded book and a resolved uuid — the
/// two pieces of playback state its active transport bar needs. `None`
/// means it should render the empty host instead: `book` alone isn't
/// enough because the Expand link would point at an empty `/listen/` route
/// (e.g. mid-dismiss, when `book` is set but `uuid` has momentarily
/// cleared).
pub(super) fn dock_active_state(
    book: Option<omnibus_shared::EbookMetadata>,
    uuid: Option<String>,
) -> Option<(omnibus_shared::EbookMetadata, String)> {
    match (book, uuid) {
        (Some(book), Some(uuid)) => Some((book, uuid)),
        _ => None,
    }
}

/// Borrowed twin of [`dock_active_state`] for callers that only need the
/// boolean — the `/read` route uses it to add the `rd-immersive` reflow class
/// exactly while the dock's bar is rendered, so the reserved stage space and
/// the visible bar can't drift apart.
pub(crate) fn dock_is_active(
    book: &Option<omnibus_shared::EbookMetadata>,
    uuid: &Option<String>,
) -> bool {
    book.is_some() && uuid.is_some()
}

/// Wall-clock listening time left at the current speed, e.g.
/// "about 1h 50m left". `None` while the duration is unknown or once playback
/// reaches/​passes the end (no phantom "1m left" at the finish); a positive
/// sub-minute remainder rounds up to "about 1m left" rather than counting
/// seconds.
pub(super) fn time_left_text(elapsed: f64, duration: f64, rate: f64) -> Option<String> {
    if duration <= 0.0 || !elapsed.is_finite() {
        return None;
    }
    let rate = if rate.is_finite() && rate > 0.0 {
        rate
    } else {
        1.0
    };
    let secs = (duration - elapsed) / rate;
    if secs <= 0.0 {
        return None;
    }
    // Bounded by duration (finite, > 0) and positive, so the cast is an
    // in-range truncation.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let mins = ((secs / 60.0).round().min(f64::from(u32::MAX))) as u64;
    let mins = mins.max(1);
    let (h, m) = (mins / 60, mins % 60);
    Some(if h > 0 {
        format!("about {h}h {m}m left")
    } else {
        format!("about {m}m left")
    })
}

/// Build the dock subtitle: the chapter label (when present), the inline
/// `elapsed / total` clock, and the speed-adjusted time left. The single-row
/// bar has no separate time track, so the clock lives here. The clock shares
/// the time-left's rate-adjusted wall-clock basis — a 1x clock beside a
/// rate-adjusted "left" disagrees with itself off 1x (#2108, matching the
/// player surfaces).
pub(super) fn dock_sub_text(
    chapter_sub: Option<String>,
    elapsed: f64,
    duration: f64,
    rate: f64,
) -> String {
    let time = format!(
        "{} / {}",
        format_hms(remaining_at_rate(elapsed, rate)),
        format_hms(remaining_at_rate(duration, rate))
    );
    let mut sub = match chapter_sub {
        Some(chapter) => format!("{chapter} \u{00b7} {time}"),
        None => time,
    };
    if let Some(left) = time_left_text(elapsed, duration, rate) {
        sub.push_str(" \u{00b7} ");
        sub.push_str(&left);
    }
    sub
}
