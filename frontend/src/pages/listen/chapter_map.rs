//! Segmented chapter progress bar doubling as a draggable seek scrubber. Each
//! chapter is a flex segment sized by `flex: {duration_seconds}`, with a
//! transparent `<input type="range">` layered on top to capture click, drag,
//! and keyboard seeking. See [`helpers::effective_scrub_position`] for the
//! drag-preview contract.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::chapter_nav::chapter_index_for_elapsed;
use super::helpers::{self, format_hms};

/// Fill percentage for a chapter segment in the progress bar.
/// `i` is the segment index, `current` is the active chapter index.
/// Returns 100 for fully played chapters, 0 for upcoming ones, and a
/// clamped proportional value for the active chapter.
pub(super) fn chapter_fill_pct(
    i: usize,
    current: usize,
    elapsed: f64,
    ch_start: f64,
    ch_duration: f64,
) -> f64 {
    if i < current {
        100.0
    } else if i == current && ch_duration > 0.0 {
        ((elapsed - ch_start) / ch_duration * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

/// Fill percentage when there are no chapter markers — falls back to a
/// single-segment proportional bar based on total duration.
pub(super) fn no_chapter_fill_pct(elapsed: f64, duration: f64) -> f64 {
    if duration > 0.0 {
        (elapsed / duration * 100.0).min(100.0)
    } else {
        0.0
    }
}

/// Horizontal position (0–100%) of the playhead thumb for `effective` seconds
/// against the scrub `max`. Clamped to the bar; 0 when `max` is non-positive.
pub(super) fn thumb_pct(effective: f64, max: f64) -> f64 {
    if max > 0.0 {
        (effective / max * 100.0).clamp(0.0, 100.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapter_fill_pct_returns_100_for_past_chapters() {
        assert!((chapter_fill_pct(0, 2, 500.0, 0.0, 200.0) - 100.0).abs() < f64::EPSILON);
        assert!((chapter_fill_pct(1, 2, 500.0, 200.0, 200.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn chapter_fill_pct_returns_0_for_upcoming_chapters() {
        assert!((chapter_fill_pct(3, 1, 300.0, 600.0, 200.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn chapter_fill_pct_returns_proportional_for_current_chapter() {
        // 50 s into a 200 s chapter → 25%
        let pct = chapter_fill_pct(1, 1, 250.0, 200.0, 200.0);
        assert!((pct - 25.0).abs() < 0.001);
    }

    #[test]
    fn chapter_fill_pct_clamps_current_chapter_at_100() {
        // elapsed past end of chapter should not exceed 100%
        let pct = chapter_fill_pct(0, 0, 999.0, 0.0, 200.0);
        assert!((pct - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn no_chapter_fill_pct_is_proportional_to_duration() {
        let pct = no_chapter_fill_pct(30.0, 120.0);
        assert!((pct - 25.0).abs() < 0.001);
    }

    #[test]
    fn no_chapter_fill_pct_returns_zero_when_duration_is_zero() {
        assert!((no_chapter_fill_pct(0.0, 0.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn thumb_pct_is_proportional_within_range() {
        assert!((thumb_pct(0.0, 200.0)).abs() < f64::EPSILON);
        assert!((thumb_pct(100.0, 200.0) - 50.0).abs() < 0.001);
        assert!((thumb_pct(200.0, 200.0) - 100.0).abs() < f64::EPSILON);
    }

    #[test]
    fn thumb_pct_clamps_past_the_end_and_below_zero() {
        assert!((thumb_pct(999.0, 200.0) - 100.0).abs() < f64::EPSILON);
        assert!((thumb_pct(-5.0, 200.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn thumb_pct_returns_zero_when_max_non_positive() {
        assert!((thumb_pct(10.0, 0.0)).abs() < f64::EPSILON);
    }
}

/// Props for the [`ChapterMap`] component.
#[derive(Props, Clone, PartialEq)]
pub(super) struct ChapterMapProps {
    /// Ordered chapter markers; empty falls back to a single-segment bar.
    pub chapters: Vec<ChapterInfo>,
    /// Seconds elapsed in the whole book.
    pub elapsed: f64,
    /// Total book duration in seconds.
    pub duration: f64,
    /// Seconds remaining in the whole book.
    pub remaining: f64,
    /// Current playback rate — the remaining readout divides by it so "time
    /// left" is wall-clock listening time, not book time.
    pub rate: f64,
    /// Index of the currently-playing chapter.
    pub current_chapter_index: usize,
    /// Fired with the target time in seconds when the bar is clicked or a
    /// drag is released.
    pub on_seek: EventHandler<f64>,
    /// True while playing but waiting on network data — shows a
    /// buffering badge below the scrub times instead of a clock that looks
    /// stuck.
    pub buffering: bool,
}

/// Scrub-state readouts derived for one render of [`ChapterMap`]: the
/// previewed position, current chapter, remaining time, thumb offset, the
/// scrub range's max (duration, or 1.0 to keep the range control non-empty
/// when duration is unknown), and whether a drag is in progress.
struct ScrubState {
    effective: f64,
    eff_current: usize,
    eff_remaining: f64,
    thumb: f64,
    max: f64,
    is_scrubbing: bool,
}

/// Derive [`ScrubState`] from the raw `scrubbing` / `scrub_secs` signal pair
/// via the shared [`helpers::effective_scrub_position`] (web's bool+f64 pair
/// normalizes to that function's `Option<f64>` shape at this one call site).
fn scrub_state(
    chapters: &[ChapterInfo],
    elapsed: f64,
    duration: f64,
    current_chapter_index: usize,
    remaining: f64,
    scrubbing: bool,
    scrub_secs: f64,
) -> ScrubState {
    let scrub = scrubbing.then_some(scrub_secs);
    let (effective, eff_current, eff_remaining) = helpers::effective_scrub_position(
        chapters,
        elapsed,
        duration,
        current_chapter_index,
        remaining,
        scrub,
        chapter_index_for_elapsed,
    );
    let max = if duration > 0.0 { duration } else { 1.0 };
    ScrubState {
        effective,
        eff_current,
        eff_remaining,
        thumb: thumb_pct(effective, max),
        max,
        is_scrubbing: scrubbing,
    }
}

/// Render the segmented chapter bar: one flex segment per chapter (or a
/// single fallback segment when there are no chapter markers), each filled
/// proportionally to how much of it has played. Pure visual — the range
/// overlay in [`ChapterMap`] owns pointer interaction (`pointer-events: none`
/// in CSS).
fn segment_bar(
    chapters: &[ChapterInfo],
    eff_current: usize,
    effective: f64,
    duration: f64,
) -> Element {
    rsx! {
        div { class: "lp-chapter-seg-row", "data-testid": "chapter-map",
            if chapters.is_empty() {
                div { class: "lp-chapter-seg", style: "flex: 1;",
                    div {
                        class: "lp-chapter-seg-fill",
                        style: "width: {no_chapter_fill_pct(effective, duration):.1}%;",
                    }
                }
            } else {
                for (i, ch) in chapters.iter().enumerate() {
                    {
                        let flex_val = ch.duration_seconds.max(0.1);
                        let fill = chapter_fill_pct(
                            i,
                            eff_current,
                            effective,
                            ch.start_seconds,
                            ch.duration_seconds,
                        );
                        let class_name = if i == eff_current {
                            "lp-chapter-seg current"
                        } else {
                            "lp-chapter-seg"
                        };
                        let title = ch.title.clone();
                        rsx! {
                            div {
                                key: "{ch.ordinal}",
                                class: "{class_name}",
                                style: "flex: {flex_val};",
                                title: "{title}",
                                div {
                                    class: "lp-chapter-seg-fill",
                                    style: "width: {fill:.1}%;",
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// The chapter progress map / seek scrubber component.
#[component]
pub(super) fn ChapterMap(props: ChapterMapProps) -> Element {
    let ChapterMapProps {
        chapters,
        elapsed,
        duration,
        remaining,
        rate,
        current_chapter_index,
        on_seek,
        buffering,
    } = props;

    // Local scrub state. While the user drags, the segments / thumb / time
    // readouts preview `scrub_secs` (see `scrub_state`); the audio element is
    // only seeked on release so dragging across a multi-part book doesn't
    // thrash `el.src`.
    let mut scrubbing = use_signal(|| false);
    let mut scrub_secs = use_signal(|| 0.0_f64);

    let ScrubState {
        effective,
        eff_current,
        eff_remaining,
        thumb,
        max,
        is_scrubbing,
    } = scrub_state(
        &chapters,
        elapsed,
        duration,
        current_chapter_index,
        remaining,
        scrubbing(),
        scrub_secs(),
    );

    let on_input = move |evt: Event<FormData>| {
        if let Ok(v) = evt.value().parse::<f64>() {
            scrub_secs.set(v);
            scrubbing.set(true);
        }
    };
    let on_change = move |evt: Event<FormData>| {
        if let Ok(v) = evt.value().parse::<f64>() {
            on_seek.call(v);
        }
        scrubbing.set(false);
    };

    let scrub_class = if is_scrubbing {
        "lp-chapter-scrub scrubbing"
    } else {
        "lp-chapter-scrub"
    };

    rsx! {
        div { class: "lp-chapter-map",
            div { class: "{scrub_class}",
                {segment_bar(&chapters, eff_current, effective, duration)}

                // Visible playhead + drag-time bubble. The bubble previews
                // the same rate-adjusted elapsed the row's left label shows,
                // so the two can't disagree mid-drag.
                div { class: "lp-chapter-thumb", style: "left: {thumb:.3}%;" }
                if is_scrubbing {
                    div {
                        class: "lp-chapter-bubble",
                        style: "left: {thumb:.3}%;",
                        "{format_hms(helpers::remaining_at_rate(effective, rate))}"
                    }
                }

                // Transparent range that captures click / drag / keyboard.
                input {
                    class: "lp-chapter-range",
                    r#type: "range",
                    "data-testid": "listen-seek",
                    "aria-label": "Seek",
                    min: "0",
                    max: "{max}",
                    step: "1",
                    value: "{effective}",
                    oninput: on_input,
                    onchange: on_change,
                }
            }

            // All three readouts share one rate-adjusted wall-clock basis
            // (elapsed + remaining = total at the current speed) — a 1x
            // elapsed or total beside a rate-adjusted remaining disagrees
            // with its own row off 1x (#2108, matching the iOS player).
            // Scaled after the scrub preview so a drag previews the
            // rate-adjusted times too (matches the mobile player).
            div { class: "lp-scrub-times",
                span { "{format_hms(helpers::remaining_at_rate(effective, rate))}" }
                span { class: "lp-scrub-remaining",
                    "\u{00b7} {format_hms(helpers::remaining_at_rate(eff_remaining, rate))} remaining"
                }
                span { "{format_hms(helpers::remaining_at_rate(duration, rate))}" }
            }
            if buffering {
                div {
                    class: "lp-buffering",
                    "data-testid": "listen-buffering",
                    role: "status",
                    "Buffering\u{2026}"
                }
            }
        }
    }
}
