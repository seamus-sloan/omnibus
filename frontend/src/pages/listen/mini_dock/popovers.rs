//! Upward popovers anchored to the mini-dock's right cluster: the popover
//! shell (always rendered, class-toggled so the open animation runs and
//! SSR/WASM node counts stay stable), the one-open-at-a-time panel state,
//! and the speed / sleep chips that host the shared panel bodies.

use dioxus::prelude::*;

use super::super::chapter_nav::chapter_index_for_elapsed;
use super::super::sleep::{end_of_chapter_seconds, sleep_chip_label};
use super::super::sleep_panel::SleepPanelBody;
use super::super::speed_panel::SpeedPanelBody;
use crate::use_playback;

/// Which right-cluster popover is open. At most one at a time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum DockPanel {
    Volume,
    Speed,
    Sleep,
}

/// Next open-state after clicking a chip: clicking the open panel's chip
/// closes it, clicking any other chip switches to that panel.
pub(super) fn toggle_panel(cur: Option<DockPanel>, clicked: DockPanel) -> Option<DockPanel> {
    if cur == Some(clicked) {
        None
    } else {
        Some(clicked)
    }
}

/// Chip/icon class with the `on` state appended while its popover is open.
pub(super) fn chip_class(base: &str, on: bool) -> String {
    if on {
        format!("{base} on")
    } else {
        base.to_string()
    }
}

/// Popover shell: always in the DOM, visibility toggled purely by the
/// `open` class so the spring animation plays and hydration node-counting
/// stays consistent (`pointer-events` is gated in CSS).
#[component]
pub(super) fn DockPopover(
    open: bool,
    testid: &'static str,
    #[props(default)] class: String,
    children: Element,
) -> Element {
    let cls = if open {
        format!("mini-dock-pop open {class}")
    } else {
        format!("mini-dock-pop {class}")
    };
    rsx! {
        div { class: "{cls}", "data-testid": testid, {children} }
    }
}

/// Speed chip + popover hosting the shared [`SpeedPanelBody`] — the same
/// preset grid / fine-tune / stepper as the full player, writing the same
/// signals.
#[component]
pub(super) fn DockSpeed(
    open_panel: Signal<Option<DockPanel>>,
    rate: f64,
    uuid: String,
    user_id: Option<i64>,
) -> Element {
    let playback = use_playback();
    let open = open_panel() == Some(DockPanel::Speed);
    let label = format!("{rate:.1}\u{00d7}");
    let on_click = move |_: MouseEvent| {
        let mut open_panel = open_panel;
        let next = toggle_panel(*open_panel.peek(), DockPanel::Speed);
        open_panel.set(next);
    };
    rsx! {
        div { class: "mini-dock-pop-anchor",
            button {
                class: chip_class("mini-dock-chip mono mini-dock-hide", open),
                r#type: "button",
                "data-testid": "mini-dock-speed",
                "aria-label": "Playback speed",
                "aria-expanded": open,
                title: "Change playback speed",
                onclick: on_click,
                "{label}"
            }
            DockPopover { open, testid: "mini-dock-pop-speed", class: "mini-dock-pop-speed",
                SpeedPanelBody {
                    rate: playback.rate,
                    rate_error: playback.rate_error,
                    user_id,
                    uuid,
                }
            }
        }
    }
}

/// Sleep chip + popover hosting the shared [`SleepPanelBody`], wired to the
/// app-scoped sleep controller — arming a timer here keeps counting after
/// navigating anywhere, including into the full player.
#[component]
pub(super) fn DockSleep(open_panel: Signal<Option<DockPanel>>) -> Element {
    let playback = use_playback();
    let sleep = super::super::sleep::use_sleep();
    let open = open_panel() == Some(DockPanel::Sleep);
    let remaining = (sleep.remaining)();
    let choice = (sleep.choice)();
    let fade = (sleep.fade)();
    let has_chapters = !playback.chapters.read().is_empty();
    let label = sleep_chip_label(remaining, choice);

    let on_click = move |_: MouseEvent| {
        let mut open_panel = open_panel;
        let next = toggle_panel(*open_panel.peek(), DockPanel::Sleep);
        open_panel.set(next);
    };
    let on_select = move |secs: i32| sleep.select_seconds(secs);
    let on_end_of_chapter = {
        let chapters = playback.chapters;
        let elapsed = playback.elapsed;
        move |_: ()| {
            let chs = chapters.peek().clone();
            let now = *elapsed.peek();
            let idx = chapter_index_for_elapsed(&chs, now);
            if let Some(secs) = end_of_chapter_seconds(&chs, idx, now) {
                sleep.select_end_of_chapter(secs);
            }
        }
    };
    let on_toggle_fade = move |_: ()| sleep.toggle_fade();

    rsx! {
        div { class: "mini-dock-pop-anchor",
            button {
                class: chip_class("mini-dock-chip mini-dock-hide", open),
                r#type: "button",
                "data-testid": "mini-dock-sleep",
                "aria-label": "Sleep timer",
                "aria-expanded": open,
                title: "Sleep timer",
                onclick: on_click,
                "{label}"
            }
            DockPopover { open, testid: "mini-dock-pop-sleep", class: "mini-dock-pop-sleep",
                SleepPanelBody {
                    remaining,
                    choice,
                    fade,
                    has_chapters,
                    on_select,
                    on_end_of_chapter,
                    on_toggle_fade,
                }
            }
        }
    }
}
