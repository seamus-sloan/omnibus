//! Persistent mini-dock audiobook bar, rendered by the web
//! [`crate::ScreenLayout`] on every main page and by the immersive
//! `/read` route (which has no transport of its own); stays absent
//! from `/listen`, whose full player already owns one. Reads the
//! app-wide [`crate::PlaybackState`] and drives transport through the
//! shared `helpers::audio_call` seam.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;

use super::helpers::format_hms;
use super::ready_player::chapter_index_for_elapsed;
use super::stage::chapter_sub_text;
use crate::components::atrium::Cover;
use crate::contexts::use_current_user_summary;
use crate::{use_playback, Route};

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

/// Playback speeds the dock's speed chip steps through on each tap.
const RATE_CYCLE: &[f64] = &[0.8, 1.0, 1.2, 1.5, 1.8, 2.0];

/// Next speed for the dock's speed chip: the first cycle value above
/// `current`, wrapping back to the slowest once past the top. A rate set
/// from the full player's fine-tune (e.g. 1.35×) advances to the next
/// preset above it.
fn cycle_rate(current: f64) -> f64 {
    RATE_CYCLE
        .iter()
        .copied()
        .find(|&r| r > current + 0.001)
        .unwrap_or(RATE_CYCLE[0])
}

/// Build the dock subtitle: the chapter label (when present) followed by the
/// inline `elapsed / total` clock. The single-row bar has no separate time
/// track, so the clock lives here.
fn dock_sub_text(chapter_sub: Option<String>, elapsed: f64, duration: f64) -> String {
    let time = format!("{} / {}", format_hms(elapsed), format_hms(duration));
    match chapter_sub {
        Some(sub) => format!("{sub} \u{00b7} {time}"),
        None => time,
    }
}

/// Persistent bottom dock. Renders only its empty host wrapper until an
/// audiobook is loaded, keeping SSR and first-WASM-paint markup identical
/// (`book` is `None` on both).
#[component]
pub fn MiniDock() -> Element {
    let playback = use_playback();
    // Declared unconditionally (before the early return) so hook order stays
    // stable across the empty-host / active-bar branches. Only drives the
    // speed chip's per-book save; never affects rendered markup, so it can't
    // cause a hydration mismatch.
    let current_user = use_current_user_summary();

    // Stable host wrapper so hydration node-counting stays consistent; only
    // the inner bar is conditional on both a loaded book and a resolved uuid.
    let Some((book, uuid)) =
        dock_active_state(playback.book.read().clone(), playback.uuid.read().clone())
    else {
        return rsx! { div { class: "mini-dock-host" } };
    };

    let elapsed = (playback.elapsed)();
    let duration = (playback.duration)();
    let playing = (playback.playing)();
    let rate = (playback.rate)();
    let chapters = (playback.chapters)();
    let user_id = current_user().map(|user| user.id);

    let idx = chapter_index_for_elapsed(&chapters, elapsed);
    let chapter_sub = chapter_sub_text(&chapters, idx);
    let sub = dock_sub_text(chapter_sub, elapsed, duration);
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let fill = format!("width: {:.1}%", progress_pct(elapsed, duration));

    rsx! {
        div { class: "mini-dock-host",
            div {
                class: "mini-dock",
                "data-testid": "mini-dock",
                role: "region",
                "aria-label": "Now playing",

                div { class: "mini-dock-prog",
                    i { style: "{fill}" }
                }

                Link {
                    to: Route::BookListen { uuid: uuid.clone(), file_id: None },
                    class: "mini-dock-now",
                    "data-testid": "mini-dock-expand",
                    "aria-label": "Expand player",
                    div { class: "mini-dock-cover", Cover { book: book.clone() } }
                    div { class: "mini-dock-meta",
                        div { class: "mini-dock-title", "data-testid": "mini-dock-title", "{title}" }
                        div { class: "mini-dock-sub", "{sub}" }
                    }
                }

                MiniDockControls { playing: playing }
                MiniDockSpeed { rate: rate, uuid: uuid.clone(), user_id: user_id }
                Link {
                    to: Route::BookListen { uuid: uuid.clone(), file_id: None },
                    class: "mini-dock-chip mini-dock-hide",
                    "data-testid": "mini-dock-sleep",
                    "aria-label": "Sleep timer \u{2014} open player",
                    title: "Sleep timer \u{2014} open player",
                    "Sleep"
                }

                span { class: "mini-dock-div" }
                MiniDockActions { uuid: uuid.clone() }
            }
        }
    }
}

/// Transport cluster: skip-back, play/pause, skip-forward. The ±30 seeks
/// carry `mini-dock-hide` so the narrow-viewport rule sheds them, leaving the
/// play button always reachable.
#[component]
fn MiniDockControls(playing: bool) -> Element {
    let play_label = if playing { "Pause" } else { "Play" }.to_string();
    let on_toggle = super::helpers::on_toggle_playback();
    let on_skip_back = super::helpers::on_skip_back_30();
    let on_skip_forward = super::helpers::on_skip_forward_30();
    rsx! {
        div { class: "mini-dock-controls",
            button {
                class: "mini-dock-btn mini-dock-skip mini-dock-hide",
                r#type: "button",
                "data-testid": "mini-dock-skip-back",
                "aria-label": "Back 30 seconds",
                onclick: on_skip_back,
                "-30"
            }
            button {
                class: "mini-dock-play",
                r#type: "button",
                "data-testid": "mini-dock-toggle",
                "aria-label": "{play_label}",
                onclick: on_toggle,
                if playing {
                    div { class: "mini-dock-ico-pause",
                        div { class: "mini-dock-ico-pause-bar" }
                        div { class: "mini-dock-ico-pause-bar" }
                    }
                } else {
                    div { class: "mini-dock-ico-play" }
                }
            }
            button {
                class: "mini-dock-btn mini-dock-skip mini-dock-hide",
                r#type: "button",
                "data-testid": "mini-dock-skip-forward",
                "aria-label": "Forward 30 seconds",
                onclick: on_skip_forward,
                "+30"
            }
        }
    }
}

/// Speed chip. Displays the current rate and cycles to the next preset on
/// click, writing through the shared `helpers::apply_rate` seam so the change
/// persists per-book and stays in sync with the full player's speed panel.
#[component]
fn MiniDockSpeed(rate: f64, uuid: String, user_id: Option<i64>) -> Element {
    let playback = use_playback();
    let label = format!("{rate:.1}\u{00d7}");
    let on_click = {
        let mut rate_sig = playback.rate;
        let rate_error = playback.rate_error;
        let uuid = uuid.clone();
        move |_: MouseEvent| {
            // Cycle from the live signal, not the render-time `rate` prop, so
            // rapid taps before a re-render each advance a full step instead of
            // recomputing the same `next` from a stale value.
            let next = cycle_rate(*rate_sig.peek());
            super::helpers::apply_rate(&mut rate_sig, rate_error, user_id, &uuid, next);
        }
    };
    rsx! {
        button {
            class: "mini-dock-chip mono mini-dock-hide",
            r#type: "button",
            "data-testid": "mini-dock-speed",
            "aria-label": "Playback speed",
            title: "Change playback speed",
            onclick: on_click,
            "{label}"
        }
    }
}

/// Inline icon actions on the right of the bar: expand to the full player and
/// dismiss. Dismiss clears [`crate::PlaybackState`] via `use_playback`.
#[component]
fn MiniDockActions(uuid: String) -> Element {
    let playback = use_playback();
    let on_dismiss = {
        let mut uuid_sig = playback.uuid;
        let mut book_sig = playback.book;
        let mut playing_set = playback.playing;
        move |_: MouseEvent| {
            // Fully stop the shared element (pause + clear src + reset mode) so a
            // media-key resume can't restart a dismissed book with no visible
            // control. Clear `book` before `uuid` so the dock hides immediately
            // and PlaybackState stays coherent for any other consumer.
            #[cfg(feature = "web")]
            super::helpers::audio_call("stop", "");
            book_sig.set(None);
            playing_set.set(false);
            uuid_sig.set(None);
        }
    };
    rsx! {
        div { class: "mini-dock-actions",
            Link {
                to: Route::BookListen { uuid: uuid.clone(), file_id: None },
                class: "mini-dock-ico",
                "data-testid": "mini-dock-expand-btn",
                "aria-label": "Expand to full player",
                title: "Expand to full player",
                svg {
                    width: "17",
                    height: "17",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.8",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M8 14l-4 4M4 14v4h4M16 10l4-4M20 10V6h-4" }
                }
            }
            button {
                class: "mini-dock-ico",
                r#type: "button",
                "data-testid": "mini-dock-dismiss",
                "aria-label": "Stop and close player",
                title: "Close audio \u{2014} keep reading",
                onclick: on_dismiss,
                svg {
                    width: "17",
                    height: "17",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.8",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M6 6l12 12M18 6L6 18" }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use omnibus_shared::EbookMetadata;

    use super::{cycle_rate, dock_active_state, dock_is_active, dock_sub_text, progress_pct};

    #[test]
    fn progress_pct_is_zero_when_duration_unknown() {
        assert_eq!(progress_pct(10.0, 0.0), 0.0);
        assert_eq!(progress_pct(10.0, -5.0), 0.0);
    }

    #[test]
    fn progress_pct_computes_midpoint() {
        assert!((progress_pct(50.0, 200.0) - 25.0).abs() < f64::EPSILON);
    }

    #[test]
    fn progress_pct_clamps_above_full() {
        assert_eq!(progress_pct(300.0, 200.0), 100.0);
    }

    #[test]
    fn progress_pct_treats_non_finite_elapsed_as_zero() {
        assert_eq!(progress_pct(f64::NAN, 100.0), 0.0);
        assert_eq!(progress_pct(f64::INFINITY, 100.0), 0.0);
    }

    #[test]
    fn cycle_rate_advances_to_next_preset() {
        assert!((cycle_rate(1.0) - 1.2).abs() < f64::EPSILON);
        assert!((cycle_rate(0.8) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn cycle_rate_wraps_after_the_fastest() {
        assert!((cycle_rate(2.0) - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn cycle_rate_advances_from_a_fine_tuned_value() {
        // 1.35× (set from the full player's slider) steps up to the next preset.
        assert!((cycle_rate(1.35) - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn cycle_rate_starts_at_slowest_from_below_the_range() {
        assert!((cycle_rate(0.5) - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn dock_sub_text_includes_chapter_and_inline_clock() {
        let sub = dock_sub_text(Some("Ch. 2 \u{00b7} The Journey".into()), 65.0, 3600.0);
        assert_eq!(sub, "Ch. 2 \u{00b7} The Journey \u{00b7} 1:05 / 1:00:00");
    }

    #[test]
    fn dock_sub_text_is_just_the_clock_without_chapters() {
        assert_eq!(dock_sub_text(None, 65.0, 130.0), "1:05 / 2:10");
    }

    // Mirrors MiniDock's "renders empty host vs. active bar" branch without needing a full component render.
    #[test]
    fn dock_active_state_is_none_when_nothing_is_playing() {
        assert_eq!(dock_active_state(None, None), None);
    }

    #[test]
    fn dock_active_state_is_none_when_uuid_has_not_resolved_yet() {
        let book = EbookMetadata {
            title: Some("Test Audiobook".into()),
            ..Default::default()
        };
        assert_eq!(dock_active_state(Some(book), None), None);
    }

    #[test]
    fn dock_active_state_is_none_when_book_has_not_loaded_yet() {
        assert_eq!(dock_active_state(None, Some("uuid-1".to_string())), None);
    }

    // The `/read` route reserves reflow space off `dock_is_active`; it must
    // agree with `dock_active_state` (the dock's own render gate) on every
    // combination or the reader reserves space for a bar that isn't there.
    #[test]
    fn dock_is_active_agrees_with_dock_active_state_for_every_combination() {
        let book = EbookMetadata {
            title: Some("Test Audiobook".into()),
            ..Default::default()
        };
        for (b, u) in [
            (None, None),
            (Some(book.clone()), None),
            (None, Some("uuid-1".to_string())),
            (Some(book.clone()), Some("uuid-1".to_string())),
        ] {
            assert_eq!(
                dock_is_active(&b, &u),
                dock_active_state(b.clone(), u.clone()).is_some()
            );
        }
    }

    #[test]
    fn dock_active_state_returns_both_when_book_and_uuid_are_present() {
        let book = EbookMetadata {
            title: Some("Test Audiobook".into()),
            ..Default::default()
        };
        let (got_book, got_uuid) =
            dock_active_state(Some(book.clone()), Some("uuid-1".to_string())).unwrap();
        assert_eq!(got_book, book);
        assert_eq!(got_uuid, "uuid-1");
    }
}
