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
/// a duration preset, and a fixed "End of ch." tag for the chapter option —
/// which fires on the playhead reaching its boundary, not on a countdown, so
/// there is no single wall-clock number the chip could honestly show for it.
pub(super) fn sleep_chip_label(remaining: Option<i32>, choice: SleepChoice) -> String {
    match (choice, remaining) {
        (SleepChoice::EndOfChapter, Some(s)) if s > 0 => "Sleep \u{00b7} End of ch.".to_string(),
        (_, Some(s)) if s > 0 => format!("Sleep \u{00b7} {}", format_countdown(s)),
        _ => "Sleep".to_string(),
    }
}

/// The book-time position an end-of-chapter timer armed at `elapsed` must
/// pause at: the end of the chapter the playhead is currently inside.
///
/// `None` when the chapter list can't place the playhead, or when the
/// playhead is already at or past that end — arming a boundary that is
/// behind the listener is what paused instantly instead of at the seam.
pub(super) fn end_of_chapter_anchor(chapters: &[ChapterInfo], elapsed: f64) -> Option<f64> {
    let idx = super::chapter_nav::chapter_index_for_elapsed(chapters, elapsed);
    let ch = chapters.get(idx)?;
    let end = ch.start_seconds + ch.duration_seconds;
    (end > elapsed).then_some(end)
}

/// Wall-clock seconds until the playhead reaches `anchor` at `rate`.
///
/// Display only. The timer fires on the playhead reaching `anchor`, never on
/// this reaching zero: a countdown cannot tell "the boundary arrived" from
/// "the listener jumped", and guessing between them is what re-armed an
/// armed timer at every seam and let the book play on for hours (#2494).
pub(super) fn seconds_until_anchor(anchor: f64, elapsed: f64, rate: f64) -> i32 {
    let rem = super::helpers::remaining_at_rate((anchor - elapsed).max(0.0), rate)
        .min(f64::from(i32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let secs = rem.ceil() as i32;
    secs
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
    /// Book-time position an armed end-of-chapter timer pauses at. The
    /// timer's authority: `remaining` is a readout derived from it, never
    /// the thing that decides.
    anchor_end: Signal<Option<f64>>,
}

impl SleepController {
    /// Select a preset (seconds). `0` turns the timer off.
    pub fn select_seconds(&self, secs: i32) {
        let mut remaining = self.remaining;
        let mut choice = self.choice;
        let mut token = self.token;
        let next_token = (*token.peek()).wrapping_add(1);
        token.set(next_token);
        let mut anchor_end = self.anchor_end;
        anchor_end.set(None);
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
        let now = *self.elapsed.peek();
        let Some(anchor) = end_of_chapter_anchor(&self.chapters.peek(), now) else {
            return;
        };
        self.arm_at(anchor, now);
    }

    /// Point the timer at `anchor` (a book-time position) and refresh the
    /// readout. Shared by arming and by the seek re-anchor, which must set
    /// exactly the same state.
    fn arm_at(&self, anchor: f64, now: f64) {
        let mut remaining = self.remaining;
        let mut choice = self.choice;
        let mut token = self.token;
        let mut anchor_end = self.anchor_end;
        let next_token = (*token.peek()).wrapping_add(1);
        token.set(next_token);
        anchor_end.set(Some(anchor));
        remaining.set(Some(
            seconds_until_anchor(anchor, now, *self.rate.peek()).max(1),
        ));
        choice.set(SleepChoice::EndOfChapter);
    }

    /// Pause playback, restore the listener's volume, and disarm. The one
    /// place the timer fires, so the fade can never complete without the
    /// stop it was ramping toward (#2494).
    fn expire(&self) {
        let restore_to = *self.volume.peek();
        #[cfg(feature = "web")]
        {
            super::helpers::audio_call("pause", "");
            super::helpers::audio_call("setVolume", &restore_to.to_string());
        }
        let _ = restore_to;
        let mut remaining = self.remaining;
        let mut choice = self.choice;
        let mut anchor_end = self.anchor_end;
        remaining.set(None);
        anchor_end.set(None);
        choice.set(SleepChoice::Off);
    }

    /// Undo a partial fade. Called whenever the timer stops counting toward
    /// the boundary it was ramping into — a re-anchor after a seek — so a
    /// listener is never left at 3% volume with a full-looking readout.
    fn restore_volume(&self) {
        let restore_to = *self.volume.peek();
        #[cfg(feature = "web")]
        super::helpers::audio_call("setVolume", &restore_to.to_string());
        let _ = restore_to;
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
    let seek_epoch = playback.seek_epoch;
    let remaining = use_signal(|| None::<i32>);
    let choice = use_signal(|| SleepChoice::Off);
    let fade = use_signal(|| true);
    let token = use_signal(|| 0u32);
    let anchor_end = use_signal(|| None::<f64>);
    let ctl = SleepController {
        remaining,
        choice,
        fade,
        token,
        volume,
        chapters,
        elapsed,
        rate,
        anchor_end,
    };

    // The end-of-chapter timer fires on POSITION, not on a countdown
    // reaching zero. `elapsed` reports several times a second while the
    // countdown ticks once, so the playhead crosses the seam while the
    // countdown still reads 1 or 2 — which is why deriving the boundary
    // from wherever the playhead happens to be re-armed the timer to the
    // next chapter forever instead of stopping the book (#2494).
    use_effect(move || {
        let now = elapsed();
        let rate_now = rate();
        if !matches!(*choice.peek(), SleepChoice::EndOfChapter) {
            return;
        }
        let Some(anchor) = *anchor_end.peek() else {
            return;
        };
        if now >= anchor {
            ctl.expire();
            return;
        }
        let mut remaining = remaining;
        remaining.set(Some(seconds_until_anchor(anchor, now, rate_now)));
    });

    // A seek is the only thing that legitimately moves the boundary, and
    // the only thing that can be told apart from playback. Re-anchor to the
    // chapter the listener landed in, and undo any fade already under way —
    // the stop it was ramping toward is no longer the next thing to happen.
    use_effect(move || {
        let _ = seek_epoch();
        let now = *elapsed.peek();
        if !matches!(*choice.peek(), SleepChoice::EndOfChapter) {
            return;
        }
        match end_of_chapter_anchor(&chapters.peek(), now) {
            Some(anchor) => {
                ctl.arm_at(anchor, now);
                ctl.restore_volume();
            }
            // Seeked past the last chapter's end: nothing left to play to,
            // so honour the timer rather than leaving it armed at a
            // boundary behind the playhead.
            None => ctl.expire(),
        }
    });

    use_effect(move || {
        let Some(secs) = remaining() else {
            return;
        };
        let fade_on = fade();
        // The chapter timer is owned by the position effect above: it fires
        // when the playhead reaches the anchor, and its readout is derived,
        // so it must neither expire on this countdown nor tick itself down
        // (a paused book reaches no boundary, and its timer should wait).
        let positional = matches!(*choice.peek(), SleepChoice::EndOfChapter);

        // Final-stretch volume ramp, scaled to the listener's own level —
        // ramping to an absolute 1.0 raised the volume of anyone not at
        // full before fading them out.
        #[cfg(feature = "web")]
        if fade_on && (0..=FADE_SECONDS).contains(&secs) {
            let target = *volume.peek();
            let v = target * (f64::from(secs) / f64::from(FADE_SECONDS)).clamp(0.0, 1.0);
            super::helpers::audio_call("setVolume", &v.to_string());
        }
        let _ = fade_on;

        if positional {
            return;
        }

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

    ctl
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

    // Three chapters: 0..300, 300..900, 900..1500.
    fn three_chapters() -> Vec<ChapterInfo> {
        vec![ch(0.0, 300.0), ch(300.0, 600.0), ch(900.0, 600.0)]
    }

    #[test]
    fn end_of_chapter_anchor_is_the_end_of_the_chapter_under_the_playhead() {
        let chs = three_chapters();
        assert_eq!(end_of_chapter_anchor(&chs, 120.0), Some(300.0));
        assert_eq!(end_of_chapter_anchor(&chs, 420.0), Some(900.0));
        assert_eq!(end_of_chapter_anchor(&chs, 1000.0), Some(1500.0));
    }

    #[test]
    fn end_of_chapter_anchor_never_names_a_boundary_behind_the_playhead() {
        // Arming past the last chapter's end would otherwise pause instantly
        // at the arming position instead of at a seam (#2494 AC5).
        let chs = three_chapters();
        assert_eq!(end_of_chapter_anchor(&chs, 1500.0), None);
        assert_eq!(end_of_chapter_anchor(&chs, 9999.0), None);
    }

    #[test]
    fn end_of_chapter_anchor_is_none_when_the_chapter_list_cannot_place_the_playhead() {
        assert_eq!(end_of_chapter_anchor(&[], 420.0), None);
    }

    #[test]
    fn end_of_chapter_anchor_does_not_move_as_the_chapter_plays_out() {
        // The whole regression in one assertion: the boundary is fixed at
        // arming and re-derived only on a seek, so every position inside the
        // chapter names the same seam. Deriving it per position report is
        // what walked the timer into the next chapter at the seam.
        let chs = three_chapters();
        for now in [301.0, 500.0, 880.0, 899.9] {
            assert_eq!(end_of_chapter_anchor(&chs, now), Some(900.0));
        }
    }

    #[test]
    fn seconds_until_anchor_counts_wall_time_at_the_playback_rate() {
        assert_eq!(seconds_until_anchor(900.0, 420.0, 1.0), 480);
        // The same 480 book-seconds play out in 240 wall-seconds at 2x.
        assert_eq!(seconds_until_anchor(900.0, 420.0, 2.0), 240);
        // A non-positive rate falls back to the unscaled remainder.
        assert_eq!(seconds_until_anchor(900.0, 420.0, 0.0), 480);
    }

    #[test]
    fn seconds_until_anchor_floors_at_zero_once_the_playhead_arrives() {
        assert_eq!(seconds_until_anchor(900.0, 900.0, 1.0), 0);
        assert_eq!(seconds_until_anchor(900.0, 901.0, 1.0), 0);
    }

    #[test]
    fn presets_start_with_off_and_cover_four_hours() {
        assert_eq!(PRESETS.first(), Some(&("Off", 0)));
        assert_eq!(PRESETS.last(), Some(&("4 hours", 14400)));
    }
}
