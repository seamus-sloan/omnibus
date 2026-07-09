//! App-wide mobile playback context.
//!
//! Owned by [`crate::App`] via `use_context_provider` (see
//! `use_user_and_playback_contexts`) so the full player, the persistent
//! mini-player, and the app-root audio host share one set of signals —
//! the mobile analogue of `contexts::PlaybackState` on web. The backing
//! `<audio>` element lives in `document.body` (installed by
//! [`super::interop`]), so playback survives route changes; these signals
//! are what let the UI keep tracking it after the player unmounts.

use dioxus::prelude::*;

use super::view::PlayerView;

/// Sleep-timer state. Countdown presets tick on wall clock (host-driven);
/// end-of-chapter fires when playback crosses the armed boundary.
#[derive(Clone, Copy, PartialEq)]
pub enum SleepState {
    Off,
    /// Wall-clock countdown: `remaining` seconds tick down once per second;
    /// `preset` is the option (in seconds) that armed it, for the sheet
    /// highlight.
    Countdown {
        remaining: i32,
        preset: i32,
    },
    /// Pause when playback reaches `at_seconds` (the current chapter's end
    /// at arm time).
    EndOfChapter {
        at_seconds: f64,
    },
}

/// App-wide mobile audiobook playback state. `uuid` is the *request*: the
/// listen page retargets it on mount and the app-root
/// [`super::host::MobileAudioHost`] reacts — loading the manifest,
/// installing the JS audio surface, and draining its events into the
/// remaining signals. Cheap to copy; every field is a `Signal`.
#[derive(Copy, Clone)]
pub struct MobilePlayback {
    pub uuid: Signal<Option<String>>,
    pub view: Signal<Option<PlayerView>>,
    pub loading: Signal<bool>,
    pub error: Signal<Option<String>>,
    /// True when the loaded book is HLS-only (not playable on mobile).
    pub unsupported: Signal<bool>,
    pub duration: Signal<f64>,
    pub elapsed: Signal<f64>,
    pub playing: Signal<bool>,
    pub rate: Signal<f64>,
    pub sleep: Signal<SleepState>,
}

impl MobilePlayback {
    /// Construct the initial (nothing-playing) state.
    pub fn new() -> Self {
        Self {
            uuid: Signal::new(None),
            view: Signal::new(None),
            loading: Signal::new(false),
            error: Signal::new(None),
            unsupported: Signal::new(false),
            duration: Signal::new(0.0),
            elapsed: Signal::new(0.0),
            playing: Signal::new(false),
            rate: Signal::new(1.0),
            sleep: Signal::new(SleepState::Off),
        }
    }
}

impl Default for MobilePlayback {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience accessor for the mobile playback context.
pub fn use_mobile_playback() -> MobilePlayback {
    use_context::<MobilePlayback>()
}

/// Sleep-preset table shown in the sheet grid. `0` seconds == "Off".
/// Mirrors the web panel's `sleep::PRESETS`.
pub(super) const SLEEP_PRESETS: &[(&str, i32)] = &[
    ("Off", 0),
    ("15 min", 900),
    ("30 min", 1800),
    ("45 min", 2700),
    ("1 hour", 3600),
    ("2 hours", 7200),
    ("3 hours", 10800),
    ("4 hours", 14400),
];

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

/// Seconds left on the sleep timer for display, deriving the end-of-chapter
/// variant from the live playback position. `None` when off.
pub(super) fn sleep_remaining(sleep: SleepState, elapsed: f64) -> Option<i32> {
    match sleep {
        SleepState::Off => None,
        SleepState::Countdown { remaining, .. } => Some(remaining.max(0)),
        SleepState::EndOfChapter { at_seconds } => {
            let rem = (at_seconds - elapsed).max(0.0).min(f64::from(i32::MAX));
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(rem.ceil() as i32)
        }
    }
}

/// Sleep-pill label for the player's secondary row: bare when off, live
/// countdown when armed.
pub(super) fn sleep_pill_label(sleep: SleepState, elapsed: f64) -> String {
    match sleep_remaining(sleep, elapsed) {
        Some(s) if s > 0 => format!("Sleep \u{00b7} {}", format_countdown(s)),
        _ => "Sleep".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_countdown_renders_m_ss_and_h_mm_ss() {
        assert_eq!(format_countdown(0), "0:00");
        assert_eq!(format_countdown(1722), "28:42");
        assert_eq!(format_countdown(7325), "2:02:05");
        assert_eq!(format_countdown(-9), "0:00");
    }

    #[test]
    fn sleep_remaining_is_none_when_off() {
        assert_eq!(sleep_remaining(SleepState::Off, 100.0), None);
    }

    #[test]
    fn sleep_remaining_reads_countdown_directly() {
        let s = SleepState::Countdown {
            remaining: 120,
            preset: 900,
        };
        assert_eq!(sleep_remaining(s, 0.0), Some(120));
    }

    #[test]
    fn sleep_remaining_derives_end_of_chapter_from_elapsed() {
        let s = SleepState::EndOfChapter { at_seconds: 500.0 };
        assert_eq!(sleep_remaining(s, 380.0), Some(120));
        // Past the boundary clamps to zero.
        assert_eq!(sleep_remaining(s, 900.0), Some(0));
    }

    #[test]
    fn sleep_pill_label_shows_countdown_when_armed() {
        assert_eq!(sleep_pill_label(SleepState::Off, 0.0), "Sleep");
        let s = SleepState::Countdown {
            remaining: 1722,
            preset: 1800,
        };
        assert_eq!(sleep_pill_label(s, 0.0), "Sleep \u{00b7} 28:42");
    }

    #[test]
    fn sleep_presets_start_with_off_and_cover_four_hours() {
        assert_eq!(SLEEP_PRESETS.first(), Some(&("Off", 0)));
        assert_eq!(SLEEP_PRESETS.last(), Some(&("4 hours", 14400)));
    }
}
