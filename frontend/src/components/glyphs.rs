//! Inline SVG marks for the "continue reading / listening" affordances — the
//! user-menu resume row and the landing hero CTA. Shared so both surfaces draw
//! the same glyph, sized to whatever pill they sit in.

use dioxus::prelude::*;

/// Solid play triangle for the "continue listening" affordance.
pub fn play_glyph(size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}", height: "{size}", view_box: "0 0 15 15", fill: "currentColor",
            "aria-hidden": "true",
            path { d: "M4 2.5v10l8-5z" }
        }
    }
}

/// Open-book outline for the "continue reading" affordance.
pub fn book_glyph(size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
        }
    }
}
