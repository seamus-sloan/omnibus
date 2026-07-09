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

/// Placement + payload for the selection popover: the live selection's screen
/// rect plus the CFI/text the action handlers fire with.
#[derive(Clone, PartialEq)]
pub(crate) struct SelectionAnchor {
    pub(crate) sel_rect_x: f64,
    pub(crate) sel_rect_y: f64,
    pub(crate) sel_rect_width: f64,
    pub(crate) sel_cfi: String,
    pub(crate) sel_text: String,
}

/// Action handlers for the selection popover — one per swatch/button.
#[derive(Clone, PartialEq)]
pub(crate) struct SelectionActions {
    pub(crate) on_dismiss: EventHandler<MouseEvent>,
    pub(crate) on_highlight: EventHandler<(String, HighlightColor, String)>,
    pub(crate) on_note: EventHandler<(String, String)>,
    pub(crate) on_copy: EventHandler<String>,
    pub(crate) on_quote: EventHandler<(String, String)>,
    pub(crate) on_share: EventHandler<String>,
}

/// Floating popover shown over a text selection: highlight color swatches
/// plus Note / Copy / Share actions.
#[component]
pub(crate) fn SelectionPopover(anchor: SelectionAnchor, actions: SelectionActions) -> Element {
    let SelectionAnchor {
        sel_rect_x,
        sel_rect_y,
        sel_rect_width,
        sel_cfi,
        sel_text,
    } = anchor;
    let SelectionActions {
        on_dismiss,
        on_highlight,
        on_note,
        on_copy,
        on_quote,
        on_share,
    } = actions;
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
                    svg {
                        width: "15", height: "15", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M12 20h9M16.5 3.5a2.1 2.1 0 0 1 3 3L7 19l-4 1 1-4L16.5 3.5z" }
                    }
                    span { "Note" }
                }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-copy",
                    onclick: {
                        let text = sel_text.clone();
                        move |_| on_copy.call(text.clone())
                    },
                    svg {
                        width: "15", height: "15", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        rect { x: "9", y: "9", width: "12", height: "12", rx: "2" }
                        path { d: "M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" }
                    }
                    span { "Copy" }
                }
                button {
                    // `rd-act-accent`: the phone popover tints Quote as the
                    // primary action, per the mobile design.
                    class: "rd-act rd-act-accent",
                    r#type: "button",
                    "data-testid": "selection-quote",
                    onclick: {
                        let cfi = sel_cfi.clone();
                        let text = sel_text.clone();
                        move |_| on_quote.call((cfi.clone(), text.clone()))
                    },
                    svg {
                        width: "15", height: "15", view_box: "0 0 24 24",
                        fill: "currentColor", stroke: "none",
                        path { d: "M10 8c0 3.5-2 6-5 7l-.8-1.6c1.7-.8 2.7-2 3-3.4H4V4h6v4zm10 0c0 3.5-2 6-5 7l-.8-1.6c1.7-.8 2.7-2 3-3.4H14V4h6v4z" }
                    }
                    span { "Quote" }
                }
                button {
                    class: "rd-act",
                    r#type: "button",
                    "data-testid": "selection-share",
                    onclick: {
                        let text = sel_text.clone();
                        move |_| on_share.call(text.clone())
                    },
                    svg {
                        width: "15", height: "15", view_box: "0 0 24 24",
                        fill: "none", stroke: "currentColor",
                        stroke_width: "1.7", stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M12 3v13M7 8l5-5 5 5M5 14v5a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2v-5" }
                    }
                    span { "Share" }
                }
            }
        }
    }
}
