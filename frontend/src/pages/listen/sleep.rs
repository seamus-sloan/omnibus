//! App-scoped sleep-timer state machine for audiobook playback. Owns the
//! countdown signal and the self-re-arming 1 s tick, plus pure helpers (preset
//! table, countdown formatting, end-of-chapter math). Installed once at App
//! root so the countdown survives leaving `/listen`, and consumed via
//! [`use_sleep`] by both the full player and the mini-dock. Session-only.

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
/// a duration preset, and a fixed "End of ch." tag for the chapter option.
/// The chapter timer's *pause point* now follows the playhead
/// ([`reanchored_end_of_chapter`]), but its countdown is only corrected
/// while the position is moving — a paused player's would still drift down,
/// so the tag stays a tag.
pub(super) fn sleep_chip_label(remaining: Option<i32>, choice: SleepChoice) -> String {
    match (choice, remaining) {
        (SleepChoice::EndOfChapter, Some(s)) if s > 0 => "Sleep \u{00b7} End of ch.".to_string(),
        (_, Some(s)) if s > 0 => format!("Sleep \u{00b7} {}", format_countdown(s)),
        _ => "Sleep".to_string(),
    }
}

/// Wall-clock seconds left until the end of the current chapter, for the
/// "End of chapter" option. The countdown ticks in real time while the book
/// plays at `rate`, so the book-time remainder is divided by the rate —
/// otherwise a 2x listener's timer fires long after the chapter has ended.
/// `None` when `idx` is out of range (empty/stale list).
pub(super) fn end_of_chapter_seconds(
    chapters: &[ChapterInfo],
    idx: usize,
    elapsed: f64,
    rate: f64,
) -> Option<i32> {
    let ch = chapters.get(idx)?;
    let end = ch.start_seconds + ch.duration_seconds;
    let rem =
        super::helpers::remaining_at_rate((end - elapsed).max(0.0), rate).min(f64::from(i32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    Some(rem.ceil() as i32)
}

/// How far the countdown may sit from the true remainder before it is
/// treated as pointing at the wrong chapter. Ordinary playback keeps the two
/// within a second of each other (both advance one wall-clock second per
/// tick); only a seek, a chapter jump or a speed change opens a real gap.
const REANCHOR_TOLERANCE_SECONDS: i32 = 3;

/// The countdown an armed end-of-chapter timer should be showing, given where
/// the playhead now is — or `None` to leave the tick alone.
///
/// The timer is a plain countdown seeded once at arming, so a listener who
/// jumps to a different chapter (or skips within one, or changes speed) is
/// left counting toward a boundary that is no longer the one ahead of them,
/// and playback runs straight through the seam. Correcting the countdown on
/// divergence rather than re-deriving it on every position report is what
/// keeps a *paused* player counting down exactly as it did before: `elapsed`
/// stops moving there, so this never runs.
pub(super) fn reanchored_end_of_chapter(
    chapters: &[ChapterInfo],
    counting_down_from: i32,
    elapsed: f64,
    rate: f64,
) -> Option<i32> {
    let idx = super::chapter_nav::chapter_index_for_elapsed(chapters, elapsed);
    let fresh = end_of_chapter_seconds(chapters, idx, elapsed, rate)?;
    ((fresh - counting_down_from).abs() > REANCHOR_TOLERANCE_SECONDS).then_some(fresh)
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
    /// Playback position, chapter map and speed. Held here so the
    /// end-of-chapter option reads them at the moment it is armed rather
    /// than being handed a value each call site computed for itself.
    chapters: Signal<Vec<ChapterInfo>>,
    elapsed: Signal<f64>,
    rate: Signal<f64>,
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

    /// Cancel any armed timer — reset to Off and restore the target volume.
    /// The countdown is app-scoped so it outlives `/listen`; without this a
    /// timer armed before "Stop and close player" keeps counting with no
    /// player open (#2353). Equivalent to selecting the "Off" preset.
    pub fn cancel(&self) {
        self.select_seconds(0);
    }

    /// Arm the timer to fire at the end of the chapter now playing. A no-op
    /// when the chapter list can't place the playhead — arming a countdown
    /// off a stale or empty list would pause at an arbitrary moment.
    pub fn select_end_of_chapter(&self) {
        let chs = self.chapters.peek().clone();
        let now = *self.elapsed.peek();
        let idx = super::chapter_nav::chapter_index_for_elapsed(&chs, now);
        let Some(secs) = end_of_chapter_seconds(&chs, idx, now, *self.rate.peek()) else {
            return;
        };
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
/// countdown outlives `/listen`. Both effects are declared unconditionally
/// (hook-order parity across SSR and WASM); only the audio interop and the
/// 1 s tick are web-gated. The signals come from the shared
/// [`crate::PlaybackState`] — the fade restores to its `volume` instead of a
/// hardcoded `1.0`, and the chapter map, position and speed are what the
/// end-of-chapter option is armed from and re-anchored against.
pub(crate) fn use_sleep_timer(playback: &crate::PlaybackState) -> SleepController {
    let volume = playback.volume;
    let chapters = playback.chapters;
    let elapsed = playback.elapsed;
    let rate = playback.rate;
    let remaining = use_signal(|| None::<i32>);
    let choice = use_signal(|| SleepChoice::Off);
    let fade = use_signal(|| true);
    let token = use_signal(|| 0u32);

    // Keep the end-of-chapter countdown pointed at the chapter that is
    // actually playing. Armed, it was a fixed countdown seeded from the
    // chapter under the playhead at that instant: jumping to another chapter
    // left it counting toward a boundary the listener had already left, and
    // playback ran straight through the new chapter's end.
    use_effect(move || {
        let now = elapsed();
        let rate_now = rate();
        // Read (not clone) the chapter list: this runs on every position
        // report, several times a second, for a timer that is usually off.
        // The read is unconditional so the effect keeps its subscription.
        let fresh = {
            let chs = chapters.read();
            if !matches!(*choice.peek(), SleepChoice::EndOfChapter) {
                return;
            }
            // A countdown at or below zero is mid-expiry, and the playhead is
            // at the seam: re-anchoring here would hand the timer the *next*
            // chapter's length instead of letting it pause.
            let Some(cur) = *remaining.peek() else {
                return;
            };
            if cur <= 0 {
                return;
            }
            reanchored_end_of_chapter(&chs, cur, now, rate_now)
        };
        if let Some(fresh) = fresh {
            let mut remaining = remaining;
            remaining.set(Some(fresh));
        }
    });

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
        chapters,
        elapsed,
        rate,
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
        assert_eq!(end_of_chapter_seconds(&chs, 1, 420.0, 1.0), Some(480));
    }

    #[test]
    fn end_of_chapter_seconds_scales_to_wall_clock_at_playback_rate() {
        let chs = vec![ch(0.0, 300.0), ch(300.0, 600.0)];
        // 480 book-seconds left plays out in 240 wall-seconds at 2x.
        assert_eq!(end_of_chapter_seconds(&chs, 1, 420.0, 2.0), Some(240));
        // A non-positive rate falls back to the unscaled remainder.
        assert_eq!(end_of_chapter_seconds(&chs, 1, 420.0, 0.0), Some(480));
    }

    #[test]
    fn end_of_chapter_seconds_floors_at_zero_past_end() {
        let chs = vec![ch(0.0, 300.0)];
        assert_eq!(end_of_chapter_seconds(&chs, 0, 999.0, 1.0), Some(0));
    }

    #[test]
    fn end_of_chapter_seconds_none_for_out_of_range_index() {
        let chs = vec![ch(0.0, 300.0)];
        assert_eq!(end_of_chapter_seconds(&chs, 5, 10.0, 1.0), None);
    }

    // Three chapters: 0..300, 300..900, 900..1500.
    fn three_chapters() -> Vec<ChapterInfo> {
        vec![ch(0.0, 300.0), ch(300.0, 600.0), ch(900.0, 600.0)]
    }

    #[test]
    fn reanchored_end_of_chapter_follows_a_jump_to_an_earlier_chapter() {
        let chs = three_chapters();
        // Armed 250 s from the end of chapter 3, then sent back to 27 s
        // before chapter 1's end. The timer must now count to *that* seam.
        assert_eq!(reanchored_end_of_chapter(&chs, 250, 273.0, 1.0), Some(27));
    }

    #[test]
    fn reanchored_end_of_chapter_follows_a_skip_inside_the_same_chapter() {
        let chs = three_chapters();
        // Still in chapter 2, but 400 s further along than the countdown
        // believes — a skip forward is as stale as a chapter jump.
        assert_eq!(reanchored_end_of_chapter(&chs, 480, 820.0, 1.0), Some(80));
    }

    #[test]
    fn reanchored_end_of_chapter_leaves_ordinary_playback_alone() {
        let chs = three_chapters();
        // One tick on from a countdown of 480: the true remainder is 479,
        // inside the jitter band, so the tick keeps the countdown.
        assert_eq!(reanchored_end_of_chapter(&chs, 480, 421.0, 1.0), None);
        assert_eq!(reanchored_end_of_chapter(&chs, 480, 423.0, 1.0), None);
    }

    #[test]
    fn reanchored_end_of_chapter_follows_a_speed_change() {
        let chs = three_chapters();
        // The same 480 book-seconds play out in 240 wall-seconds at 2x, so
        // a countdown armed at 1x is twice as long as the wait really is.
        assert_eq!(reanchored_end_of_chapter(&chs, 480, 420.0, 2.0), Some(240));
    }

    #[test]
    fn reanchored_end_of_chapter_is_none_when_the_chapter_list_cannot_place_the_playhead() {
        assert_eq!(reanchored_end_of_chapter(&[], 480, 420.0, 1.0), None);
    }

    #[test]
    fn presets_start_with_off_and_cover_four_hours() {
        assert_eq!(PRESETS.first(), Some(&("Off", 0)));
        assert_eq!(PRESETS.last(), Some(&("4 hours", 14400)));
    }
}
