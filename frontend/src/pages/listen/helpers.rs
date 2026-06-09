//! Shared helpers for the listen page: timestamp formatting, the audio
//! progress POST shim, and the audited `window.OmnibusAudio` poke.
//! Consumed by `listen.rs`, `controls`, `speed_panel`, and `bootstrap`.

#[cfg(feature = "web")]
use dioxus::prelude::*;

/// Vendored hls.js for the HLS fallback path.
#[cfg(feature = "web")]
pub(super) const HLS_JS: Asset = asset!("/assets/vendor/hls.min.js");

/// Single audited surface for poking `window.OmnibusAudio`.
#[cfg(feature = "web")]
pub(super) fn audio_call(method: &str, arg_js: &str) {
    let js = format!("window.OmnibusAudio && window.OmnibusAudio.{method}({arg_js});");
    let _ = dioxus::document::eval(&js);
}

/// Format `seconds` as `H:MM:SS` (or `MM:SS` when under an hour).
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) fn format_hms(seconds: f64) -> String {
    if !seconds.is_finite() || seconds < 0.0 {
        return "0:00".into();
    }
    let s_total = seconds as u64;
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
