//! App-scoped sleep-timer state machine for audiobook playback.
//!
//! Owns the countdown signal and the self-re-arming 1 s tick, plus pure
//! helpers (preset table, countdown formatting, end-of-chapter math). The
//! controller is installed once at App root (so the countdown survives
//! leaving `/listen`) and consumed via [`use_sleep`] by both the full
//! player and the mini-dock. Session-only — not persisted across page loads.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

/// Duration presets shown in the panel grid. `0` seconds == "Off".
pub(super) const PRESETS: &[(&str, i32)] = &[
    ("Off", 0),
    ("15 min", 900),
    ("30 min", 1800),
    ("45 min", 2700),
    ("1 hour", 3600),
    ("2 hours", 7200),
    ("3 hours", 10800),
    ("4 hours", 14400),
];

/// Seconds over which the volume ramps to zero before the timer fires.
/// Only read by the web-gated fade ramp in [`use_sleep_timer`].
#[cfg(feature = "web")]
const FADE_SECONDS: i32 = 30;

/// Which sleep option is currently selected — drives the panel highlight.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SleepChoice {
    Off,
    Preset(i32),
    EndOfChapter,
}

/// Format a non-negative countdown as `M:SS` (or `H:MM:SS` past an hour).
pub(super) fn format_countdown(secs: i32) -> String {
    let s = secs.max(0);
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

/// Toolbar Sleep-button label, showing the live countdown when active.
pub(super) fn sleep_toolbar_label(remaining: Option<i32>) -> String {
    match remaining {
        Some(s) if s > 0 => format!("Sleep \u{00b7} {}", format_countdown(s)),
        _ => "Sleep \u{00b7} off".to_string(),
    }
}

/// Mini-dock Sleep-chip label: bare "Sleep" when off, the live countdown for
/// a duration preset, and a fixed "End of ch." tag for the chapter option
/// (its remaining seconds drift with seeks, so a countdown would mislead).
pub(super) fn sleep_chip_label(remaining: Option<i32>, choice: SleepChoice) -> String {
    match (choice, remaining) {
        (SleepChoice::EndOfChapter, Some(s)) if s > 0 => "Sleep \u{00b7} End of ch.".to_string(),
        (_, Some(s)) if s > 0 => format!("Sleep \u{00b7} {}", format_countdown(s)),
        _ => "Sleep".to_string(),
    }
}

/// Seconds left until the end of the current chapter, for the "End of
/// chapter" option. `None` when `idx` is out of range (empty/stale list).
pub(super) fn end_of_chapter_seconds(
    chapters: &[ChapterInfo],
    idx: usize,
    elapsed: f64,
) -> Option<i32> {
    let ch = chapters.get(idx)?;
    let end = ch.start_seconds + ch.duration_seconds;
    let rem = (end - elapsed).max(0.0).min(f64::from(i32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(rem.ceil() as i32)
}

/// Sleep-timer handle returned by [`use_sleep_timer`]. Cheap to copy; the
/// controller methods bump an internal session token so an in-flight tick
/// from a cancelled session is ignored.
#[derive(Clone, Copy)]
pub(crate) struct SleepController {
    pub remaining: Signal<Option<i32>>,
    pub choice: Signal<SleepChoice>,
    pub fade: Signal<bool>,
    token: Signal<u32>,
    /// The user's target volume — the fade restores to this, not always
    /// `1.0`, so cancelling/expiring the timer doesn't fight the volume
    /// slider's chosen level.
    volume: Signal<f64>,
}

impl SleepController {
    /// Select a preset (seconds). `0` turns the timer off.
    pub fn select_seconds(&self, secs: i32) {
        let mut remaining = self.remaining;
        let mut choice = self.choice;
        let mut token = self.token;
        let next_token = (*token.peek()).wrapping_add(1);
        token.set(next_token);
        if secs <= 0 {
            remaining.set(None);
            choice.set(SleepChoice::Off);
            let restore_to = *self.volume.peek();
            #[cfg(feature = "web")]
            super::helpers::audio_call("setVolume", &restore_to.to_string());
            let _ = restore_to;
        } else {
            remaining.set(Some(secs));
            choice.set(SleepChoice::Preset(secs));
        }
    }

    /// Arm the timer to fire at the end of the current chapter.
    pub fn select_end_of_chapter(&self, secs: i32) {
        let mut remaining = self.remaining;
        let mut choice = self.choice;
        let mut token = self.token;
        let next_token = (*token.peek()).wrapping_add(1);
        token.set(next_token);
        remaining.set(Some(secs.max(1)));
        choice.set(SleepChoice::EndOfChapter);
    }

    /// Toggle the volume-fade preference. Turning it off restores the
    /// user's target volume (not `1.0`).
    pub fn toggle_fade(&self) {
        let mut fade = self.fade;
        let was_on = *fade.peek();
        fade.set(!was_on);
        if was_on {
            let restore_to = *self.volume.peek();
            #[cfg(feature = "web")]
            super::helpers::audio_call("setVolume", &restore_to.to_string());
            let _ = restore_to;
        }
    }
}

/// App-scoped accessor for the [`SleepController`] provided at App root.
pub(crate) fn use_sleep() -> SleepController {
    use_context()
}

/// Install the sleep-timer signals and the self-re-arming countdown effect.
/// Called once from App root (`use_user_and_playback_contexts`) so the
/// countdown outlives `/listen`. The effect is declared unconditionally
/// (hook-order parity across SSR and WASM); only the audio interop and the
/// 1 s tick are web-gated. `volume` is the shared
/// [`crate::PlaybackState::volume`] signal — the fade restores to it instead
/// of a hardcoded `1.0`.
pub(crate) fn use_sleep_timer(volume: Signal<f64>) -> SleepController {
    let remaining = use_signal(|| None::<i32>);
    let choice = use_signal(|| SleepChoice::Off);
    let fade = use_signal(|| true);
    let token = use_signal(|| 0u32);

    use_effect(move || {
        let Some(secs) = remaining() else {
            return;
        };
        let fade_on = fade();

        // Final-stretch volume ramp.
        #[cfg(feature = "web")]
        if fade_on && (0..=FADE_SECONDS).contains(&secs) {
            let v = (f64::from(secs) / f64::from(FADE_SECONDS)).clamp(0.0, 1.0);
            super::helpers::audio_call("setVolume", &v.to_string());
        }
        let _ = fade_on;

        if secs <= 0 {
            // Expiry: pause playback and restore the user's target volume
            // for next time (not always 1.0).
            let restore_to = *volume.peek();
            #[cfg(feature = "web")]
            {
                super::helpers::audio_call("pause", "");
                super::helpers::audio_call("setVolume", &restore_to.to_string());
            }
            let _ = restore_to;
            let mut remaining = remaining;
            let mut choice = choice;
            remaining.set(None);
            choice.set(SleepChoice::Off);
        } else {
            // Arm the next tick, guarded by the session token so a cancelled
            // session's in-flight sleep does not keep decrementing.
            #[cfg(feature = "web")]
            {
                let my_token = *token.peek();
                let mut remaining = remaining;
                let token = token;
                spawn(async move {
                    gloo_timers::future::sleep(std::time::Duration::from_secs(1)).await;
                    if *token.peek() != my_token {
                        return;
                    }
                    let cur = *remaining.peek();
                    if let Some(c) = cur {
                        remaining.set(Some(c - 1));
                    }
                });
            }
        }
    });

    SleepController {
        remaining,
        choice,
        fade,
        token,
        volume,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal: 1,
            title: "x".into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    #[test]
    fn format_countdown_under_one_hour_is_m_ss() {
        assert_eq!(format_countdown(0), "0:00");
        assert_eq!(format_countdown(5), "0:05");
        assert_eq!(format_countdown(125), "2:05");
        assert_eq!(format_countdown(1722), "28:42");
    }

    #[test]
    fn format_countdown_past_one_hour_is_h_mm_ss() {
        assert_eq!(format_countdown(3600), "1:00:00");
        assert_eq!(format_countdown(7325), "2:02:05");
    }

    #[test]
    fn format_countdown_clamps_negative_to_zero() {
        assert_eq!(format_countdown(-9), "0:00");
    }

    #[test]
    fn sleep_toolbar_label_off_when_none_or_zero() {
        assert_eq!(sleep_toolbar_label(None), "Sleep \u{00b7} off");
        assert_eq!(sleep_toolbar_label(Some(0)), "Sleep \u{00b7} off");
    }

    #[test]
    fn sleep_toolbar_label_shows_countdown_when_active() {
        assert_eq!(sleep_toolbar_label(Some(1722)), "Sleep \u{00b7} 28:42");
    }

    #[test]
    fn sleep_chip_label_is_bare_sleep_when_off() {
        assert_eq!(sleep_chip_label(None, SleepChoice::Off), "Sleep");
        assert_eq!(sleep_chip_label(Some(0), SleepChoice::Off), "Sleep");
    }

    #[test]
    fn sleep_chip_label_shows_countdown_for_preset() {
        assert_eq!(
            sleep_chip_label(Some(1722), SleepChoice::Preset(1800)),
            "Sleep \u{00b7} 28:42"
        );
    }

    #[test]
    fn sleep_chip_label_names_end_of_chapter_instead_of_counting_down() {
        assert_eq!(
            sleep_chip_label(Some(311), SleepChoice::EndOfChapter),
            "Sleep \u{00b7} End of ch."
        );
    }

    #[test]
    fn end_of_chapter_seconds_returns_remaining_in_current_chapter() {
        let chs = vec![ch(0.0, 300.0), ch(300.0, 600.0)];
        // 120 s into chapter 2 (start 300, dur 600 → ends 900); elapsed 420 → 480 left.
        assert_eq!(end_of_chapter_seconds(&chs, 1, 420.0), Some(480));
    }

    #[test]
    fn end_of_chapter_seconds_floors_at_zero_past_end() {
        let chs = vec![ch(0.0, 300.0)];
        assert_eq!(end_of_chapter_seconds(&chs, 0, 999.0), Some(0));
    }

    #[test]
    fn end_of_chapter_seconds_none_for_out_of_range_index() {
        let chs = vec![ch(0.0, 300.0)];
        assert_eq!(end_of_chapter_seconds(&chs, 5, 10.0), None);
    }

    #[test]
    fn presets_start_with_off_and_cover_four_hours() {
        assert_eq!(PRESETS.first(), Some(&("Off", 0)));
        assert_eq!(PRESETS.last(), Some(&("4 hours", 14400)));
    }
}
