//! Text-selection event payload from the epub.js glue plus the floating
//! highlight popover rendered over a live selection.

use dioxus::prelude::*;

use omnibus_shared::HighlightColor;

/// Selection event data from epub.js glue (deserialized from JSON).
//
// INVARIANT: full serde shape preserved for forward-compat with the
// epub.js glue payload. `text` isn't surfaced in the popover today, but
// the glue always sends it and downstream highlight-export work needs
// it; thinning the schema now would force a re-add when that lands.
#[derive(Clone, Default, serde::Deserialize)]
#[allow(dead_code)]
pub(crate) struct SelectionData {
    #[serde(rename = "cfiRange")]
    pub(crate) cfi_range: String,
    pub(crate) text: String,
    pub(crate) rect: SelectionRect,
}

// INVARIANT: full rect kept for serde round-trip even though `height`
// isn't read — the popover only positions horizontally today, but the
// glue always emits all four fields and a vertical-flip placement is on
// deck.
#[derive(Clone, Default, serde::Deserialize)]
#[allow(dead_code)]
pub(crate) struct SelectionRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
    pub(crate) height: f64,
}

/// Floating popover shown over a text selection with highlight color swatches.
#[component]
pub(crate) fn SelectionPopover(
    sel_rect_x: f64,
    sel_rect_y: f64,
    sel_rect_width: f64,
    sel_cfi: String,
    on_dismiss: EventHandler<MouseEvent>,
    on_highlight: EventHandler<(String, HighlightColor)>,
) -> Element {
    let top = (sel_rect_y - 52.0).max(8.0);
    let left = (sel_rect_x + sel_rect_width / 2.0 - 90.0).clamp(8.0, 600.0);
    let style = format!("top:{top}px; left:{left}px;");

    let colors = [
        (HighlightColor::Amber, "amber"),
        (HighlightColor::Green, "green"),
        (HighlightColor::Blue, "blue"),
        (HighlightColor::Rose, "rose"),
        (HighlightColor::Violet, "violet"),
    ];

    rsx! {
        div { class: "rd-scrim rd-scrim-clear", onclick: on_dismiss }
        div {
            class: "rd-selection-popover",
            style: "{style}",
            onclick: move |evt: MouseEvent| evt.stop_propagation(),
            div {
                class: "rd-swatch-row",
                for (color, name) in colors {
                    {
                        let cfi = sel_cfi.clone();
                        rsx! {
                            button {
                                class: "rd-swatch",
                                r#type: "button",
                                "data-color": "{name}",
                                "aria-label": "Highlight {name}",
                                onclick: move |_| on_highlight.call((cfi.clone(), color)),
                            }
                        }
                    }
                }
            }
        }
    }
}
