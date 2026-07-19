//! Centered transport cluster for the mini-dock: whole-chapter jumps,
//! ±30 s seeks, and the play/pause button. All actions write through the
//! shared `helpers` seam so the dock and the full player stay in lockstep.

use dioxus::prelude::*;

use super::super::chapter_nav::{
    chapter_index_for_elapsed, chapter_next_target, chapter_prev_target,
};
use super::super::helpers;
use crate::use_playback;

/// Transport cluster, absolutely centered in the bar:
/// `[⟨CH] [-30] [play] [+30] [CH⟩]`. The chapter jumps and ±30 seeks carry
/// `mini-dock-hide` so the narrow-viewport rule sheds them, leaving the play
/// button always reachable.
#[component]
pub(super) fn DockTransport(playing: bool, has_chapters: bool) -> Element {
    let playback = use_playback();
    let play_label = if playing { "Pause" } else { "Play" }.to_string();
    let on_toggle = helpers::on_toggle_playback();
    let on_skip_back = helpers::on_skip_back_30();
    let on_skip_forward = helpers::on_skip_forward_30();
    // Chapter jumps peek the live signals inside the handler (not render-time
    // props) so a click always works from the current position, mirroring the
    // full player's handlers in `ready_player`.
    let on_chapter_prev = {
        let chapters = playback.chapters;
        let elapsed = playback.elapsed;
        move |_: MouseEvent| {
            let chs = chapters.peek().clone();
            let now = *elapsed.peek();
            let idx = chapter_index_for_elapsed(&chs, now);
            if let Some(target) = chapter_prev_target(&chs, now, idx) {
                helpers::seek_to(target);
            }
        }
    };
    let on_chapter_next = {
        let chapters = playback.chapters;
        let elapsed = playback.elapsed;
        move |_: MouseEvent| {
            let chs = chapters.peek().clone();
            let idx = chapter_index_for_elapsed(&chs, *elapsed.peek());
            if let Some(target) = chapter_next_target(&chs, idx) {
                helpers::seek_to(target);
            }
        }
    };

    rsx! {
        div { class: "mini-dock-mid",
            button {
                class: "mini-dock-ch mini-dock-hide",
                r#type: "button",
                disabled: !has_chapters,
                "data-testid": "mini-dock-ch-prev",
                "aria-label": "Previous chapter",
                title: "Previous chapter",
                onclick: on_chapter_prev,
                span { class: "lp-ch-bar" }
                span { class: "lp-ch-tri-left" }
            }
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
            button {
                class: "mini-dock-ch mini-dock-hide",
                r#type: "button",
                disabled: !has_chapters,
                "data-testid": "mini-dock-ch-next",
                "aria-label": "Next chapter",
                title: "Next chapter",
                onclick: on_chapter_next,
                span { class: "lp-ch-tri-right" }
                span { class: "lp-ch-bar" }
            }
        }
    }
}
