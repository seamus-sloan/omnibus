//! Shared helpers for the listen page: timestamp formatting, the audio
//! progress POST shim, the audited `window.OmnibusAudio` poke, the
//! transport click-handler builders, and the volume load/save/apply trio
//! shared by the mini-dock and full player. Consumed by `listen.rs`,
//! `controls`, `speed_panel`, `sleep`, `ready_player`, and `bootstrap`.

// Imported for `Asset`/`MouseEvent`; the audio helpers below that use it are
// all gated to non-mobile targets, so the import is too (unused on mobile).
#[cfg(not(feature = "mobile"))]
use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::{
    MAX_AUDIOBOOK_PLAYBACK_RATE as MAX_RATE, MIN_AUDIOBOOK_PLAYBACK_RATE as MIN_RATE,
};

/// Vendored hls.js for the HLS fallback path.
#[cfg(feature = "web")]
pub(super) const HLS_JS: Asset = asset!("/assets/vendor/hls.min.js");

/// Single audited surface for poking `window.OmnibusAudio`.
#[cfg(feature = "web")]
pub(super) fn audio_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusAudio && window.OmnibusAudio.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

// The transport handlers are shared by the mini-dock and full player, both of
// which are `#![cfg(not(feature = "mobile"))]`, so gate the builders to match
// — on mobile they'd be dead code and fail clippy's `-D warnings`. Each is a
// no-op on non-web (SSR) targets, where there is no audio element to poke.

/// Click handler that toggles play/pause via `window.OmnibusAudio`.
#[cfg(not(feature = "mobile"))]
pub(super) fn on_toggle_playback() -> impl FnMut(MouseEvent) + 'static {
    move |_: MouseEvent| {
        #[cfg(feature = "web")]
        audio_call("toggle", "");
    }
}

/// Click handler that skips back 30 seconds.
#[cfg(not(feature = "mobile"))]
pub(super) fn on_skip_back_30() -> impl FnMut(MouseEvent) + 'static {
    move |_: MouseEvent| {
        #[cfg(feature = "web")]
        audio_call("skip", "-30");
    }
}

/// Click handler that skips forward 30 seconds.
#[cfg(not(feature = "mobile"))]
pub(super) fn on_skip_forward_30() -> impl FnMut(MouseEvent) + 'static {
    move |_: MouseEvent| {
        #[cfg(feature = "web")]
        audio_call("skip", "30");
    }
}

/// Seek the shared audio element to an absolute position in seconds. Shared
/// by the full player's scrub/chapter handlers and the mini-dock's chapter
/// jumps; no-op off web (SSR has no audio element to poke).
#[cfg(not(feature = "mobile"))]
pub(super) fn seek_to(secs: f64) {
    #[cfg(feature = "web")]
    audio_call("seek", &secs.to_string());
    #[cfg(not(feature = "web"))]
    let _ = secs;
}

/// localStorage key for the persisted session volume preference.
#[cfg(feature = "web")]
const VOLUME_KEY: &str = "omnibus.listen.volume";

/// Clamp a requested volume to the `<audio>` element's valid range,
/// treating non-finite input (NaN, +/-inf) as full volume.
#[cfg(not(feature = "mobile"))]
pub(super) fn clamp_volume(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        1.0
    }
}

/// Load the persisted volume preference. Defaults to `1.0` when unset,
/// unparsable, or off web — the volume slider itself is web-only, so
/// mobile/SSR never need a stored value. Its only production caller
/// (`bootstrap::install_audio_bootstrap`) is itself web-gated, so this
/// would read as dead code on a server-only build without the allow.
#[cfg(not(feature = "mobile"))]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub(super) fn load_volume() -> f64 {
    #[cfg(feature = "web")]
    {
        crate::client_store::get(VOLUME_KEY)
            .and_then(|raw| raw.parse::<f64>().ok())
            .map(clamp_volume)
            .unwrap_or(1.0)
    }
    #[cfg(not(feature = "web"))]
    {
        1.0
    }
}

/// Persist the volume preference. No-op outside web.
#[cfg(not(feature = "mobile"))]
fn save_volume(v: f64) {
    #[cfg(feature = "web")]
    crate::client_store::set(VOLUME_KEY, &v.to_string());
    #[cfg(not(feature = "web"))]
    let _ = v;
}

/// Clamp `v`, persist it as the session-wide volume preference, update the
/// shared signal, and forward it to the JS audio shim so the `<audio>`
/// element's volume changes in real time. Shared by the full player's
/// slider and the mini-dock's compact slider — both write through here so
/// they always agree.
#[cfg(not(feature = "mobile"))]
pub(super) fn apply_volume(volume: &mut Signal<f64>, v: f64) {
    let clamped = clamp_volume(v);
    volume.set(clamped);
    save_volume(clamped);
    #[cfg(feature = "web")]
    audio_call("setVolume", &clamped.to_string());
}

/// Fine-tune granularity for playback rate — the speed panel's stepper and
/// the mini-dock's "is this preset active" check both snap to this.
#[cfg(not(feature = "mobile"))]
pub(super) const RATE_STEP: f64 = 0.05;

/// Clamp `new_rate` to `[MIN_RATE, MAX_RATE]` and snap to the nearest
/// `RATE_STEP`. Pure logic extracted for unit testing — no signal or JS
/// side-effects.
#[cfg(not(feature = "mobile"))]
fn clamp_and_round_rate(new_rate: f64) -> f64 {
    let clamped = new_rate.clamp(MIN_RATE, MAX_RATE);
    (clamped / RATE_STEP).round() * RATE_STEP
}

/// Snap/clamp `new_rate`, update the shared signal, persist it per-book, and
/// forward it to the JS audio shim so playback speed changes in real time.
/// Shared by the full player's speed panel and the mini-dock's speed chip so
/// the two always agree. `rate_error` surfaces a save failure to the UI.
#[cfg(not(feature = "mobile"))]
pub(super) fn apply_rate(
    rate: &mut Signal<f64>,
    rate_error: Signal<Option<String>>,
    user_id: Option<i64>,
    uuid: &str,
    new_rate: f64,
) {
    let rounded = clamp_and_round_rate(new_rate);
    rate.set(rounded);
    if let Some(user_id) = user_id {
        crate::audiobook_progress::save_rate(user_id, uuid, rounded);
    }
    #[cfg(feature = "web")]
    audio_call("setRate", &rounded.to_string());
    let uuid = uuid.to_string();
    spawn(async move {
        let mut rate_error = rate_error;
        let update = omnibus_shared::AudiobookPlaybackRateUpdate {
            playback_rate: rounded,
        };
        match crate::data::set_playback_rate("", &uuid, update).await {
            Ok(_) => rate_error.set(None),
            Err(error) => rate_error.set(Some(format!("Could not save playback speed: {error}"))),
        }
    });
}

/// Format `seconds` as `H:MM:SS` (or `MM:SS` when under an hour). On mobile
/// the sleep-timer countdown delegates here (`mobile::state::format_countdown`).
pub(super) fn format_hms(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    // Cap to ~136 years so the cast cannot wrap. Combined with the
    // finite/non-negative check above, the `as u64` is an in-range
    // truncation, never a saturation to `u64::MAX`.
    let bounded = seconds.min(f64::from(u32::MAX));
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let s_total = bounded as u64;
    let h = s_total / 3600;
    let m = (s_total % 3600) / 60;
    let s = s_total % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Fire-and-forget POST `/api/rpc/progress` with the audio update.
#[cfg(feature = "web")]
pub(super) fn post_audio_progress(uuid: String, seconds: f64) {
    wasm_bindgen_futures::spawn_local(async move {
        let body = serde_json::json!({
            "update": {
                "book_uuid": uuid,
                "format": "audio",
                "audio_position_seconds": seconds,
            }
        });
        if let Ok(req) = gloo_net::http::Request::post("/api/rpc/progress").json(&body) {
            let _ = req.send().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::format_hms;

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
}

// `clamp_volume`/`load_volume`/`clamp_and_round_rate` don't exist on mobile
// (the slider and speed panel are web-only), so their tests live in a
// separately-gated module.
#[cfg(all(test, not(feature = "mobile")))]
mod volume_tests {
    use super::{clamp_and_round_rate, clamp_volume, load_volume, MAX_RATE, MIN_RATE};

    #[test]
    fn clamp_and_round_rate_accepts_value_within_range() {
        assert!((clamp_and_round_rate(1.0) - 1.0).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn clamp_and_round_rate_clamps_below_minimum() {
        assert!((clamp_and_round_rate(0.0) - MIN_RATE).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn clamp_and_round_rate_clamps_above_maximum() {
        assert!((clamp_and_round_rate(10.0) - MAX_RATE).abs() < f64::EPSILON * 10.0);
    }

    #[test]
    fn clamp_and_round_rate_rounds_to_nearest_step() {
        assert!((clamp_and_round_rate(1.07) - 1.05).abs() < 0.001);
        assert!((clamp_and_round_rate(1.08) - 1.10).abs() < 0.001);
    }

    #[test]
    fn clamp_volume_within_range_is_unchanged() {
        assert!((clamp_volume(0.42) - 0.42).abs() < f64::EPSILON);
    }

    #[test]
    fn clamp_volume_clamps_outside_zero_to_one() {
        assert_eq!(clamp_volume(-0.5), 0.0);
        assert_eq!(clamp_volume(1.5), 1.0);
    }

    #[test]
    fn clamp_volume_treats_non_finite_as_full_volume() {
        assert_eq!(clamp_volume(f64::NAN), 1.0);
        assert_eq!(clamp_volume(f64::INFINITY), 1.0);
        assert_eq!(clamp_volume(f64::NEG_INFINITY), 1.0);
    }

    #[test]
    fn load_volume_defaults_to_one_under_ssr() {
        // The `web` localStorage path isn't reachable under the `server`
        // feature this test runs with — pins the documented SSR default.
        assert!((load_volume() - 1.0).abs() < f64::EPSILON);
    }
}
