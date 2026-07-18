//! Volume control for the mini-dock: an icon button opening an upward
//! popover with a vertical slider. Writes through `helpers::apply_volume`
//! so the dock, the full player, and the sleep-timer fade all share one
//! volume signal.

use dioxus::prelude::*;

use super::super::helpers::apply_volume;
use super::popovers::{chip_class, toggle_panel, DockPanel, DockPopover};
use crate::use_playback;

/// Volume icon + vertical-slider popover.
#[component]
pub(super) fn DockVolume(open_panel: Signal<Option<DockPanel>>, volume: f64) -> Element {
    let playback = use_playback();
    let open = open_panel() == Some(DockPanel::Volume);
    let pct = (volume * 100.0).round();
    let fill = format!("--p: {pct:.0}%;");
    let on_click = move |_: MouseEvent| {
        let mut open_panel = open_panel;
        let next = toggle_panel(*open_panel.peek(), DockPanel::Volume);
        open_panel.set(next);
    };
    let on_input = {
        let mut volume_sig = playback.volume;
        move |evt: Event<FormData>| {
            if let Ok(v) = evt.value().parse::<f64>() {
                apply_volume(&mut volume_sig, v);
            }
        }
    };
    rsx! {
        div { class: "mini-dock-pop-anchor mini-dock-hide",
            button {
                class: chip_class("mini-dock-ico", open),
                r#type: "button",
                "data-testid": "mini-dock-volume",
                "aria-label": "Volume",
                "aria-expanded": open,
                title: "Volume",
                onclick: on_click,
                svg {
                    width: "17",
                    height: "17",
                    view_box: "0 0 24 24",
                    fill: "none",
                    stroke: "currentColor",
                    stroke_width: "1.7",
                    stroke_linecap: "round",
                    stroke_linejoin: "round",
                    path { d: "M4 9v6h4l5 4V5L8 9H4z", fill: "currentColor", stroke: "none" }
                    path { d: "M16.5 8.5a5 5 0 0 1 0 7" }
                }
            }
            DockPopover { open, testid: "mini-dock-pop-vol", class: "mini-dock-pop-vol",
                div { class: "mini-dock-vol-vert",
                    input {
                        r#type: "range",
                        min: "0",
                        max: "1",
                        step: "0.05",
                        value: "{volume}",
                        style: "{fill}",
                        "aria-label": "Volume",
                        oninput: on_input,
                    }
                    div { class: "mono", "{pct:.0}" }
                }
            }
        }
    }
}
