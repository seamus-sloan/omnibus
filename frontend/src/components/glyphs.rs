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

/// Headphones outline for the audiobook progress row on the book detail
/// "Your progress" block.
pub fn headphones_glyph(size: u32) -> Element {
    rsx! {
        svg {
            width: "{size}", height: "{size}", view_box: "0 0 16 16", fill: "none",
            stroke: "currentColor", stroke_width: "1.35", stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": "true",
            path { d: "M3 10V8.5a5 5 0 0 1 10 0V10" }
            path {
                d: "M2.75 10h2.5v3.75h-1.5a1 1 0 0 1-1-1zM13.25 10h-2.5v3.75h1.5a1 1 0 0 0 1-1z",
                fill: "currentColor",
            }
        }
    }
}

/// Magnifying-glass search icon, reused by the search palette's trigger
/// button and query input, and the book-detail merge dialog's search box.
/// `class` carries the caller's own sizing/positioning hook; `aria_hidden`
/// matches each call site's existing markup (the palette's two uses render
/// it undecorated, the merge dialog marks it decorative).
pub fn search_glyph(size: u32, class: &str, aria_hidden: bool) -> Element {
    rsx! {
        svg {
            class: "{class}",
            width: "{size}", height: "{size}", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            "aria-hidden": if aria_hidden { Some("true") } else { None },
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}
