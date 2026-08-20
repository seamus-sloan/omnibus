//! The cross-format sync mark: the paired-arrows glyph every sync surface
//! wears (entry row, modal, prompts, hero), ported from the Format Sync
//! design so the surfaces read as one feature.

use dioxus::prelude::*;

/// The bidirectional-arrows glyph. `size` is the square edge in px.
#[component]
pub fn SyncGlyph(#[props(default = 14)] size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}",
            height: "{size}",
            view_box: "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "1.8",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            "aria-hidden": "true",
            path { d: "M4 9h13.5" }
            path { d: "M13.5 5.5 17.5 9l-4 3.5" }
            path { d: "M20 15H6.5" }
            path { d: "M10.5 18.5 6.5 15l4-3.5" }
        }
    }
}
