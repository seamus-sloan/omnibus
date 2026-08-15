//! Shared helpers for the listen page: timestamp formatting, the audio
//! progress POST shim, the audited `window.OmnibusAudio` poke, the
//! transport click-handler builders, the volume load/save/apply trio
//! shared by the mini-dock and full player, and the scrub-drag preview
//! math shared by the web and mobile players.

// Imported for `Asset`/`MouseEvent`; the audio helpers below that use it are
// all gated to non-mobile targets, so the import is too (unused on mobile).
#[cfg(not(feature = "mobile"))]
use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::{
    MAX_AUDIOBOOK_PLAYBACK_RATE as MAX_RATE, MIN_AUDIOBOOK_PLAYBACK_RATE as MIN_RATE,
};

/// Effective (position, chapter index, remaining) derivation shared by the
/// web and mobile players' scrub bars: while dragging (`scrub` is `Some`)
/// every readout previews the target position instead of the real playback
/// state; at rest (`None`) `elapsed` / `current_chapter_index` / `remaining`
/// pass through unchanged. `chapter_index_fn` is threaded in rather than
/// called directly because each target keeps its own
/// `chapter_index_for_elapsed` under its own cfg gate (`chapter_nav` on web,
/// `mobile::view` on mobile) — this is the one place the "preview while
/// dragging, commit on release" math itself lives, so the two can't diverge.
///
/// Note the `target.clamp(0.0, max)` below is a deliberate, small behavior
/// change on mobile: the pre-refactor mobile derivation used the raw drag
/// value unclamped (`scrub().unwrap_or(elapsed)`), while web already
/// clamped. A stale drag value that outlives a mid-drag `duration` change
/// (or any other out-of-range input) now clamps into range on both
/// platforms instead of only on web — intentionally aligning mobile with
/// web's existing, safer edge-case behavior rather than preserving mobile's
/// unguarded one.
pub(super) fn effective_scrub_position(
    chapters: &[ChapterInfo],
    elapsed: f64,
    duration: f64,
    current_chapter_index: usize,
    remaining: f64,
    scrub: Option<f64>,
    chapter_index_fn: impl Fn(&[ChapterInfo], f64) -> usize,
) -> (f64, usize, f64) {
    let Some(target) = scrub else {
        return (elapsed, current_chapter_index, remaining);
    };
    let max = if duration > 0.0 { duration } else { 1.0 };
    let effective = target.clamp(0.0, max);
    let eff_current = chapter_index_fn(chapters, effective);
    let eff_remaining = (duration - effective).max(0.0);
    (effective, eff_current, eff_remaining)
}

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
/// by the full player's scrub/chapter/bookmark handlers and the mini-dock's
/// chapter jumps; no-op off web (SSR has no audio element to poke). The JS
/// `seek` shim pushes the target time to the transport display immediately
/// — playing or paused — rather than waiting on the element's own
/// `timeupdate` (#1897).
#[cfg(not(feature = "mobile"))]
pub(super) fn seek_to(secs: f64) {
    #[cfg(feature = "web")]
    audio_call("seek", &secs.to_string());
    #[cfg(not(feature = "web"))]
    let _ = secs;
}

// The resume-decision trio below is called from both the web bootstrap
// (`bootstrap::run_manifest_init`) and the mobile host driver
// (`mobile::host::load_manifest`/`init_direct_and_drain`) — un-gated on
// either target so it can't drift between the two (#1888, #1923).

/// Select the resume position: prefer the server-authoritative value when
/// available, fall back to the locally cached initial position.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
fn resolve_resume_pos(server_pos: Option<f64>, local_pos: f64) -> f64 {
    server_pos.unwrap_or(local_pos)
}

/// The file to request the manifest with: an explicit picker selection wins;
/// otherwise the progress row's stored `book_file_id`, so resume lands in
/// the file the seconds were recorded in (#1888); `None` (the server's
/// lowest-ordinal default) only when neither names one.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
pub(super) fn resolve_boot_file(requested: Option<i64>, row_file: Option<i64>) -> Option<i64> {
    requested.or(row_file)
}

/// Seconds to seed playback with, given the file the manifest actually
/// resolved. On a single-file book — or when a legacy server reports no
/// file identity (`audio_file_count == 0`) — this is the historic behavior:
/// server seconds, else the locally cached position. On a multi-file book
/// an offset is applied only when the progress row names the loaded file:
/// seconds recorded in another file, or in no named file at all (the local
/// cache never names one), start playback at zero rather than splicing one
/// file's offset into another (#1888).
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
pub(super) fn resolve_boot_position(
    row: Option<(Option<i64>, Option<f64>)>,
    local_pos: f64,
    loaded_file_id: i64,
    audio_file_count: i64,
) -> Option<f64> {
    if audio_file_count <= 1 {
        return Some(resolve_resume_pos(
            row.and_then(|(_, seconds)| seconds),
            local_pos,
        ));
    }
    match row {
        Some((Some(row_file), Some(seconds))) if row_file == loaded_file_id => Some(seconds),
        // Seconds recorded in another file (or in none) can't be spliced
        // into this one (#1888) — and `None` here means UNSEEDED, not
        // "position 0": a zero seed marks the element's 0 as a real
        // position, which the transport then persists over the row the
        // seconds actually live in (#1954's data-loss arm).
        _ => None,
    }
}

/// Follow-at-boot override: with follow on and a mapped Candidate, the
/// boot seeds the mapped `(file, seconds)` instead of the stored row — a
/// post-boot corrective seek would race `initDirect`'s one-shot restore
/// seek and lose. An explicit picker `?file_id=` outranks follow: that is
/// a deliberate navigation to a specific file, not a resume.
#[cfg(not(feature = "mobile"))]
#[cfg_attr(not(feature = "web"), allow(dead_code))]
pub(super) fn resolve_follow_boot(
    explicit_file: Option<i64>,
    resume: Option<&omnibus_shared::cross_format::CrossFormatResume>,
) -> Option<(Option<i64>, f64)> {
    if explicit_file.is_some() {
        return None;
    }
    let resume = resume?;
    if !resume.follow
        || resume.state != omnibus_shared::cross_format::CrossFormatResumeState::Candidate
    {
        return None;
    }
    let c = resume.candidate.as_ref()?;
    Some((c.book_file_id, c.audio_position_seconds?))
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
    // Snap by multiplying into whole step counts and dividing back out —
    // `steps.round() * RATE_STEP` accumulates binary-float error (1.2 becomes
    // 1.2000000000000002, which then leaks into the persisted JSON payload).
    let steps_per_unit = (1.0 / RATE_STEP).round();
    (clamped * steps_per_unit).round() / steps_per_unit
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

/// Rate-adjusted "time left" for a real (1x) `remaining` duration; falls back
/// to `remaining` unscaled when `rate` is non-finite or non-positive. Shared
/// by the web and mobile players and the landing resume hero so every
/// remaining-time readout scales the same way.
pub(crate) fn remaining_at_rate(remaining: f64, rate: f64) -> f64 {
    if !rate.is_finite() || rate <= 0.0 {
        return remaining;
    }
    remaining / rate
}

/// Fire-and-forget POST `/api/rpc/progress` with the audio update.
///
/// `file_id` is the `book_files` row currently playing — the id the
/// manifest itself resolved (`PlaybackState::loaded_file_id`), so multi-file
/// writes always name their file and the Continue surfaces reopen *this*
/// audiobook rather than the first one by ordinal. `None` (no manifest
/// resolved yet, or a server predating the identity fields) omits the field;
/// the server-side guard then refuses to blank a named file on a multi-file
/// row (#1888).
#[cfg(feature = "web")]
pub(super) fn post_audio_progress(uuid: String, file_id: Option<i64>, seconds: f64) {
    wasm_bindgen_futures::spawn_local(async move {
        let mut update = serde_json::json!({
            "book_uuid": uuid,
            "format": "audio",
            "audio_position_seconds": seconds,
        });
        // Insert only when known, so this hand-built payload matches how
        // `ProgressUpdate` itself serializes (every optional field carries
        // `skip_serializing_if`) rather than sending an explicit null.
        if let (Some(file_id), Some(map)) = (file_id, update.as_object_mut()) {
            map.insert("book_file_id".into(), file_id.into());
        }
        let body = serde_json::json!({ "update": update });
        if let Ok(req) = gloo_net::http::Request::post("/api/rpc/progress").json(&body) {
            let _ = req.send().await;
        }
    });
}

#[cfg(test)]
mod tests {
    use omnibus_shared::ChapterInfo;

    use super::{effective_scrub_position, format_hms, remaining_at_rate};

    fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    /// Mirrors the shape of the real per-target `chapter_index_for_elapsed`
    /// implementations (`chapter_nav` / `mobile::view`) without depending on
    /// either — this module compiles on both cfg configurations.
    fn idx_of(chapters: &[ChapterInfo], elapsed: f64) -> usize {
        chapters
            .partition_point(|c| c.start_seconds <= elapsed)
            .saturating_sub(1)
    }

    #[test]
    fn effective_scrub_position_passes_through_when_not_scrubbing() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        let (effective, idx, remaining) =
            effective_scrub_position(&chs, 120.0, 900.0, 0, 780.0, None, idx_of);
        assert!((effective - 120.0).abs() < f64::EPSILON);
        assert_eq!(idx, 0);
        assert!((remaining - 780.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effective_scrub_position_overrides_with_drag_target_when_scrubbing() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // Parent still reports elapsed=120/chapter 0/remaining=780, but the
        // drag is previewing 500s — every readout should follow the drag.
        let (effective, idx, remaining) =
            effective_scrub_position(&chs, 120.0, 900.0, 0, 780.0, Some(500.0), idx_of);
        assert!((effective - 500.0).abs() < f64::EPSILON);
        assert_eq!(idx, 1);
        assert!((remaining - 400.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effective_scrub_position_recomputes_chapter_at_a_boundary() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
        // Dragging to exactly the chapter-2 boundary should flip the index.
        let (_, idx_before, _) =
            effective_scrub_position(&chs, 0.0, 900.0, 0, 900.0, Some(299.9), idx_of);
        let (_, idx_after, _) =
            effective_scrub_position(&chs, 0.0, 900.0, 0, 900.0, Some(300.0), idx_of);
        assert_eq!(idx_before, 0);
        assert_eq!(idx_after, 1);
    }

    #[test]
    fn effective_scrub_position_clamps_a_drag_target_past_the_end() {
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        let (effective, _, remaining) =
            effective_scrub_position(&chs, 0.0, 300.0, 0, 300.0, Some(999.0), idx_of);
        assert!((effective - 300.0).abs() < f64::EPSILON);
        assert!((remaining - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn effective_scrub_position_clamps_a_negative_drag_target_to_zero() {
        // A stale/out-of-range scrub value (e.g. one that outlives a
        // mid-drag `duration` change) clamps into range on both platforms —
        // this is the mobile-alignment fix noted on `effective_scrub_position`.
        let chs = vec![ch(1, "Intro", 0.0, 300.0)];
        let (effective, idx, remaining) =
            effective_scrub_position(&chs, 120.0, 300.0, 0, 180.0, Some(-50.0), idx_of);
        assert!((effective - 0.0).abs() < f64::EPSILON);
        assert_eq!(idx, 0);
        assert!((remaining - 300.0).abs() < f64::EPSILON);
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
    fn remaining_at_rate_divides_by_the_playback_rate() {
        assert!((remaining_at_rate(600.0, 2.0) - 300.0).abs() < f64::EPSILON);
        assert!((remaining_at_rate(600.0, 0.5) - 1200.0).abs() < f64::EPSILON);
        assert!((remaining_at_rate(600.0, 1.0) - 600.0).abs() < f64::EPSILON);
    }

    #[test]
    fn remaining_at_rate_falls_back_unscaled_for_invalid_rates() {
        assert!((remaining_at_rate(600.0, 0.0) - 600.0).abs() < f64::EPSILON);
        assert!((remaining_at_rate(600.0, -1.0) - 600.0).abs() < f64::EPSILON);
        assert!((remaining_at_rate(600.0, f64::NAN) - 600.0).abs() < f64::EPSILON);
        assert!((remaining_at_rate(600.0, f64::INFINITY) - 600.0).abs() < f64::EPSILON);
    }
}

// The resume-decision helpers are shared by both the web bootstrap and the
// mobile host driver, so their tests run under every feature combination.
#[cfg(test)]
mod boot_resume_tests {
    use omnibus_shared::cross_format::{
        CrossFormatCandidate, CrossFormatResume, CrossFormatResumeState, MappingConfidence,
    };
    use omnibus_shared::ProgressFormat;

    #[cfg(not(feature = "mobile"))]
    use super::resolve_follow_boot;
    use super::{resolve_boot_file, resolve_boot_position, resolve_resume_pos};

    // Only read by the `resolve_follow_boot` tests, which share its
    // `not(mobile)` gate.
    #[cfg_attr(feature = "mobile", allow(dead_code))]
    fn follow_resume(follow: bool, state: CrossFormatResumeState) -> CrossFormatResume {
        CrossFormatResume {
            state,
            candidate: Some(CrossFormatCandidate {
                target: ProgressFormat::Audio,
                source_format: ProgressFormat::Epub,
                source_client_updated_at: 1,
                confidence: MappingConfidence::UserAnchored,
                book_file_id: Some(917),
                audio_position_seconds: Some(21_822.0),
                total_duration_seconds: Some(35_760.0),
                percent: None,
                fraction: None,
                epub_cfi: None,
                source_position_seconds: None,
                source_ahead: Some(true),
            }),
            follow,
        }
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn resolve_follow_boot_seeds_the_mapped_file_and_seconds() {
        let resume = follow_resume(true, CrossFormatResumeState::Candidate);
        assert_eq!(
            resolve_follow_boot(None, Some(&resume)),
            Some((Some(917), 21_822.0))
        );
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn resolve_follow_boot_stands_aside_for_an_explicit_picker_file() {
        let resume = follow_resume(true, CrossFormatResumeState::Candidate);
        assert_eq!(resolve_follow_boot(Some(919), Some(&resume)), None);
    }

    #[cfg(not(feature = "mobile"))]
    #[test]
    fn resolve_follow_boot_requires_follow_and_a_candidate_state() {
        let no_follow = follow_resume(false, CrossFormatResumeState::Candidate);
        assert_eq!(resolve_follow_boot(None, Some(&no_follow)), None);
        let aligned = follow_resume(true, CrossFormatResumeState::Aligned);
        assert_eq!(resolve_follow_boot(None, Some(&aligned)), None);
        assert_eq!(resolve_follow_boot(None, None), None);
    }

    #[test]
    fn resolve_resume_pos_prefers_server_position_when_present() {
        assert!((resolve_resume_pos(Some(120.0), 5.0) - 120.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_resume_pos_falls_back_to_local_when_server_absent() {
        assert!((resolve_resume_pos(None, 42.5) - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_resume_pos_returns_zero_local_when_both_absent() {
        assert!((resolve_resume_pos(None, 0.0)).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_boot_file_prefers_explicit_picker_selection() {
        assert_eq!(resolve_boot_file(Some(917), Some(919)), Some(917));
    }

    #[test]
    fn resolve_boot_file_falls_back_to_the_progress_rows_file() {
        assert_eq!(resolve_boot_file(None, Some(919)), Some(919));
        assert_eq!(resolve_boot_file(None, None), None);
    }

    #[test]
    fn resolve_boot_position_applies_row_seconds_when_the_row_names_the_loaded_file() {
        let row = Some((Some(919), Some(23_718.0)));
        assert!((resolve_boot_position(row, 5.0, 919, 5).unwrap() - 23_718.0).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_boot_position_stays_unseeded_when_multi_file_seconds_are_unattributable() {
        // Row names a different file than the one loaded: `None` (unseeded),
        // never a zero seed the transport could persist (#1954).
        let other_file = Some((Some(919), Some(23_718.0)));
        assert_eq!(resolve_boot_position(other_file, 5.0, 917, 5), None);
        // Row names no file at all — seconds can't be attributed.
        let fileless = Some((None, Some(23_718.0)));
        assert_eq!(resolve_boot_position(fileless, 5.0, 917, 5), None);
        // No row; the local cache never names a file either.
        assert_eq!(resolve_boot_position(None, 5.0, 917, 5), None);
    }

    #[test]
    fn resolve_boot_position_keeps_single_file_resume_behavior() {
        // Server seconds win; the stored file id (even a stale one from a
        // replaced `book_files` row) doesn't gate a single-file book.
        let row = Some((Some(1), Some(120.0)));
        assert!((resolve_boot_position(row, 5.0, 2, 1).unwrap() - 120.0).abs() < f64::EPSILON);
        // No server row → locally cached position, as before.
        assert!((resolve_boot_position(None, 42.5, 2, 1).unwrap() - 42.5).abs() < f64::EPSILON);
    }

    #[test]
    fn resolve_boot_position_treats_legacy_server_without_identity_as_single_file() {
        // `audio_file_count == 0` = a server predating the identity fields;
        // degrade to the historic resume behavior rather than zeroing.
        let row = Some((None, Some(300.0)));
        assert!((resolve_boot_position(row, 5.0, 0, 0).unwrap() - 300.0).abs() < f64::EPSILON);
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
