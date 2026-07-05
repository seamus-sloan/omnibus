//! Mobile bottom tab bar.
//!
//! Pinned to the bottom of the viewport — a row of icon tabs (Library,
//! Authors, Series, You) matching the imported Atrium mobile design. It is
//! the native shell's primary `Nav`, and also mounts on web via
//! [`crate::ScreenLayout`], where CSS reveals it only below the phone
//! breakpoint so desktop keeps its top-bar section links.

use dioxus::prelude::*;
use dioxus_router::{use_route, Link};

use crate::Route;

/// The four bottom-tab destinations. `You` routes to the Account screen; the
/// Settings page is reachable from within it and keeps the You tab lit.
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabKind {
    Library,
    Authors,
    Series,
    You,
}

/// Whether `tab` should render active for the given current route. Grouped so
/// a tab lights up across its whole section (e.g. an author *detail* page
/// keeps the Authors tab lit), not only on the exact index route.
fn is_active(current: &Route, tab: TabKind) -> bool {
    match tab {
        TabKind::Library => matches!(
            current,
            Route::Landing {}
                | Route::Shelves {}
                | Route::ShelfDetail { .. }
                | Route::BookDetail { .. }
                | Route::MetadataEdit { .. }
                | Route::Search { .. }
        ),
        TabKind::Authors => matches!(current, Route::AuthorsIndex {} | Route::AuthorDetail { .. }),
        TabKind::Series => matches!(current, Route::SeriesIndex {} | Route::SeriesDetail { .. }),
        TabKind::You => matches!(
            current,
            Route::Account {} | Route::Settings {} | Route::AddBooks {}
        ),
    }
}

/// Renders the bottom navigation bar component.
#[component]
pub fn BottomNav() -> Element {
    let current = use_route::<Route>();
    rsx! {
        nav { class: "m-tabbar", aria_label: "Primary",
            MTab { to: Route::Landing {}, label: "Library", on: is_active(&current, TabKind::Library), glyph: tab_glyph_library() }
            MTab { to: Route::AuthorsIndex {}, label: "Authors", on: is_active(&current, TabKind::Authors), glyph: tab_glyph_authors() }
            MTab { to: Route::SeriesIndex {}, label: "Series", on: is_active(&current, TabKind::Series), glyph: tab_glyph_series() }
            MTab { to: Route::Account {}, label: "You", on: is_active(&current, TabKind::You), glyph: tab_glyph_you() }
        }
    }
}

/// One tab: a router link stacking a glyph over its label, lit when `on`.
#[component]
fn MTab(to: Route, label: String, on: bool, glyph: Element) -> Element {
    let class = if on {
        "m-tabbar-item on"
    } else {
        "m-tabbar-item"
    };
    rsx! {
        Link { to, class: "{class}",
            span { class: "m-tabbar-glyph", {glyph} }
            span { class: "m-tabbar-label", "{label}" }
        }
    }
}

// ── Tab glyphs (stroked SVG, currentColor) ─────────────────────────────

fn tab_glyph_library() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M4 19.5A2.5 2.5 0 0 1 6.5 17H20" }
            path { d: "M6.5 2H20v20H6.5A2.5 2.5 0 0 1 4 19.5v-15A2.5 2.5 0 0 1 6.5 2z" }
        }
    }
}

fn tab_glyph_authors() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M17 21v-2a4 4 0 0 0-4-4H7a4 4 0 0 0-4 4v2" }
            circle { cx: "10", cy: "7", r: "4" }
        }
    }
}

fn tab_glyph_series() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            rect { x: "3", y: "3", width: "13", height: "13", rx: "2" }
            path { d: "M21 8v11a2 2 0 0 1-2 2H8" }
        }
    }
}

fn tab_glyph_you() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "12", cy: "8", r: "4" }
            path { d: "M4 21v-1a6 6 0 0 1 6-6h4a6 6 0 0 1 6 6v1" }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn library_tab_lights_across_library_section() {
        assert!(is_active(&Route::Landing {}, TabKind::Library));
        assert!(is_active(&Route::Shelves {}, TabKind::Library));
        assert!(is_active(&Route::ShelfDetail { id: 3 }, TabKind::Library));
        assert!(is_active(
            &Route::BookDetail { uuid: "x".into() },
            TabKind::Library
        ));
        // Routes the mobile library/detail link into must keep the Library
        // tab lit — otherwise no tab shows active on those pages.
        assert!(is_active(
            &Route::MetadataEdit { uuid: "x".into() },
            TabKind::Library
        ));
        assert!(is_active(
            &Route::Search { query: "x".into() },
            TabKind::Library
        ));
    }

    #[test]
    fn authors_and_series_tabs_light_on_their_details() {
        assert!(is_active(&Route::AuthorsIndex {}, TabKind::Authors));
        assert!(is_active(&Route::AuthorDetail { id: 1 }, TabKind::Authors));
        assert!(is_active(&Route::SeriesIndex {}, TabKind::Series));
        assert!(is_active(&Route::SeriesDetail { id: 1 }, TabKind::Series));
    }

    #[test]
    fn you_tab_lights_across_account_section() {
        assert!(is_active(&Route::Account {}, TabKind::You));
        assert!(is_active(&Route::Settings {}, TabKind::You));
        assert!(is_active(&Route::AddBooks {}, TabKind::You));
        assert!(!is_active(&Route::Landing {}, TabKind::You));
    }

    #[test]
    fn tabs_are_mutually_exclusive_on_landing() {
        let here = Route::Landing {};
        assert!(is_active(&here, TabKind::Library));
        assert!(!is_active(&here, TabKind::Authors));
        assert!(!is_active(&here, TabKind::Series));
        assert!(!is_active(&here, TabKind::You));
    }

    #[test]
    fn account_route_lights_only_the_you_tab() {
        let here = Route::Account {};
        assert!(!is_active(&here, TabKind::Library));
        assert!(!is_active(&here, TabKind::Authors));
        assert!(!is_active(&here, TabKind::Series));
        assert!(is_active(&here, TabKind::You));
    }
}
