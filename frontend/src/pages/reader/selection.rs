//! Text-selection event payload from the epub.js glue plus the floating
//! highlight popover rendered over a live selection.

use dioxus::prelude::*;

use omnibus_shared::HighlightColor;

/// Selection event data from epub.js glue (deserialized from JSON).
#[derive(Clone, Default, serde::Deserialize)]
pub(crate) struct SelectionData {
    #[serde(rename = "cfiRange")]
    pub(crate) cfi_range: String,
    #[serde(default)]
    pub(crate) text: String,
    pub(crate) rect: SelectionRect,
}

/// Bounding rect of a live selection in viewport coordinates.
#[derive(Clone, Default, serde::Deserialize)]
pub(crate) struct SelectionRect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) width: f64,
}

/// Floating popover shown over a text selection: highlight color swatches
/// plus Note / Copy / Share actions.
#[component]
pub(crate) fn SelectionPopover(
    sel_rect_x: f64,
    sel_rect_y: f64,
    sel_rect_width: f64,
    sel_cfi: String,
    sel_text: String,
    on_dismiss: EventHandler<MouseEvent>,
    on_highlight: EventHandler<(String, HighlightColor, String)>,
    on_note: EventHandler<(String, String)>,
    on_copy: EventHandler<String>,
    on_quote: EventHandler<(String, String)>,
    on_share: EventHandler<String>,
) -> Element {
    let top = (sel_rect_y - 52.0).max(8.0);
    let left = (sel_rect_x + sel_rect_width / 2.0 - 150.0).clamp(8.0, 560.0);
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
                        let text = sel_text.clone();
                        rsx! {
                            button {
                                key: "{name}",
                                class: "rd-swatch",
                                r#type: "button",
                                "data-color": "{name}",
                                "aria-label": "Highlight {name}",
                                onclick: move |_| on_highlight.call((cfi.clone(), color, text.clone())),
                            }
                        }
                    }
                }
                span { class: "rd-pop-div" }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-note",
                    onclick: {
                        let cfi = sel_cfi.clone();
                        let text = sel_text.clone();
                        move |_| on_note.call((cfi.clone(), text.clone()))
                    },
                    "Note"
                }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-copy",
                    onclick: {
                        let text = sel_text.clone();
                        move |_| on_copy.call(text.clone())
                    },
                    "Copy"
                }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-quote",
                    onclick: {
                        let cfi = sel_cfi.clone();
                        let text = sel_text.clone();
                        move |_| on_quote.call((cfi.clone(), text.clone()))
                    },
                    "Quote"
                }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-share",
                    onclick: {
                        let text = sel_text.clone();
                        move |_| on_share.call(text.clone())
                    },
                    "Share"
                }
            }
        }
    }
}
