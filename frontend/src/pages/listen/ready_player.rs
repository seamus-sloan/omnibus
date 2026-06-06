//! Post-load player chrome: top bar, hidden `<audio>`, status overlays,
//! and the two-column [`PlayerStage`]. Owns the per-action handlers
//! (back / toggle / skip / seek / rate). Rendered by the orchestrator
//! after the `loading` / `error` / `book.is_none()` gates.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::controls::{AudioElement, TopBar};
use super::overlays::{FailedOverlay, PreparingOverlay};
use super::speed_panel::SpeedPanel;
use super::stage::PlayerStage;

/// Render the ready-state player chrome and bind the transport handlers.
#[component]
pub(super) fn ReadyPlayer(
    book: EbookMetadata,
    uuid: String,
    duration: Signal<f64>,
    elapsed: Signal<f64>,
    playing: Signal<bool>,
    rate: Signal<f64>,
    hls_ready: Signal<bool>,
    playback_failed: Signal<bool>,
) -> Element {
    let nav = use_navigator();
    let mut speed_panel_open = use_signal(|| false);

    let on_back = move |_| {
        nav.go_back();
    };

    let on_toggle = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("toggle", "");
    };
    let on_skip_back = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "-30");
    };
    let on_skip_forward = move |_| {
        #[cfg(feature = "web")]
        super::helpers::audio_call("skip", "30");
    };
    let on_seek = move |evt: Event<FormData>| {
        if let Ok(_secs) = evt.value().parse::<f64>() {
            #[cfg(feature = "web")]
            super::helpers::audio_call("seek", &_secs.to_string());
        }
    };
    let on_rate = move |_: MouseEvent| {
        let cur = *speed_panel_open.peek();
        speed_panel_open.set(!cur);
    };
    let on_speed_close = move |_| {
        speed_panel_open.set(false);
    };

    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| "Unknown Author".to_string());
    let dur = duration();
    let elapsed_now = elapsed();
    let remaining = (dur - elapsed_now).max(0.0);
    let scrub_max = if dur > 0.0 { dur } else { 1.0 };
    let rate_label = format!("{:.2}\u{00d7}", rate());
    let play_label = if playing() { "Pause" } else { "Play" }.to_string();
    let ready = hls_ready();
    let failed = playback_failed();

    let accent_style = book
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();

    rsx! {
        div { class: "lp-root", style: "{accent_style}",
            div { class: "lp-backdrop" }

            TopBar { on_back }
            AudioElement {}

            if failed {
                FailedOverlay {}
            } else if !ready {
                PreparingOverlay {}
            }

            PlayerStage {
                book: book.clone(),
                title,
                author,
                elapsed: elapsed_now,
                duration: dur,
                remaining,
                scrub_max,
                play_label,
                rate_label,
                rate_active: speed_panel_open(),
                on_seek,
                on_toggle,
                on_skip_back,
                on_skip_forward,
                on_rate,
                on_speed: move |_| speed_panel_open.set(true),
                on_sleep: move |_| {},
                on_bookmark: move |_| {},
                on_chapters: move |_| {},
            }

            if speed_panel_open() {
                SpeedPanel {
                    rate,
                    uuid: uuid.clone(),
                    on_close: on_speed_close,
                }
            }
        }
    }
}
