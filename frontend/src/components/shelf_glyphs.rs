//! Inline SVG marks for shelves — kind (smart cog, wishlist heart, hand-picked
//! bookmark), visibility (padlock, people), the add / remove actions, and the
//! "All books" stack. Shared by the landing shelves row, the shelves index,
//! and the shelf page so every surface marks a shelf the same way.

use dioxus::prelude::*;
use omnibus_shared::{ShelfKind, Visibility};

/// The kind glyph for `kind`.
pub fn kind_icon(kind: ShelfKind) -> Element {
    match kind {
        ShelfKind::Smart => cog_icon(),
        ShelfKind::Manual => bookmark_icon(),
        ShelfKind::Wishlist => heart_icon(),
    }
}

/// The visibility glyph for `visibility`.
pub fn visibility_icon(visibility: Visibility) -> Element {
    match visibility {
        Visibility::Private => lock_icon(),
        Visibility::Public => people_icon(),
    }
}

/// Plus glyph for the add actions.
pub fn plus_icon() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2.2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M12 5v14M5 12h14" }
        }
    }
}

/// Cross glyph for taking a book off a shelf.
pub fn x_icon() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2.2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M18 6 6 18M6 6l12 12" }
        }
    }
}

/// Stack-of-books glyph for the "All books" row.
pub fn all_books_icon() -> Element {
    rsx! {
        svg {
            width: "14", height: "14", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
        }
    }
}

/// Cog glyph marking a smart shelf.
pub fn cog_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "12", cy: "12", r: "3" }
            path { d: "M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 1 1-2.83 2.83l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-4 0v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 1 1-2.83-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1 0-4h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 1 1 2.83-2.83l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 4 0v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 1 1 2.83 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 0 4h-.09a1.65 1.65 0 0 0-1.51 1z" }
        }
    }
}

/// Heart glyph marking the built-in Wishlist shelf.
pub fn heart_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M20.84 4.61a5.5 5.5 0 0 0-7.78 0L12 5.67l-1.06-1.06a5.5 5.5 0 0 0-7.78 7.78l1.06 1.06L12 21.23l7.78-7.78 1.06-1.06a5.5 5.5 0 0 0 0-7.78z" }
        }
    }
}

/// Bookmark glyph marking a hand-picked shelf.
pub fn bookmark_icon() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" }
        }
    }
}

/// Padlock glyph marking a private shelf.
pub fn lock_icon() -> Element {
    rsx! {
        svg {
            width: "12", height: "12", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            rect { x: "3", y: "11", width: "18", height: "11", rx: "2", ry: "2" }
            path { d: "M7 11V7a5 5 0 0 1 10 0v4" }
        }
    }
}

/// Two-people glyph marking a public shelf.
pub fn people_icon() -> Element {
    rsx! {
        svg {
            width: "13", height: "13", view_box: "0 0 24 24",
            fill: "none", stroke: "currentColor", stroke_width: "2",
            stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M17 21v-2a4 4 0 0 0-4-4H5a4 4 0 0 0-4 4v2" }
            circle { cx: "9", cy: "7", r: "4" }
            path { d: "M23 21v-2a4 4 0 0 0-3-3.87" }
            path { d: "M16 3.13a4 4 0 0 1 0 7.75" }
        }
    }
}
