//! Mobile bottom tab bar: four icon tabs (Library, Authors, Stats, You) split
//! around a raised center action that opens the add-books chooser. The native
//! shell's primary `Nav`; also mounts on web via `ScreenLayout`, hidden by CSS
//! above the phone breakpoint so desktop keeps its top-bar links.

use dioxus::prelude::*;
use dioxus_router::{use_route, Link};

use super::add_books_sheet::AddBooksSheet;
use crate::Route;

/// The four bottom-tab destinations. `You` routes to the Account screen; the
/// Settings page is reachable from within it and keeps the You tab lit. Series
/// left the tab bar for the center action (still reachable via search + author
/// pages + the `/series` route).
#[derive(Clone, Copy, PartialEq, Eq)]
enum TabKind {
    Library,
    Authors,
    Stats,
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
        TabKind::Stats => matches!(current, Route::Stats {}),
        TabKind::You => matches!(
            current,
            Route::Account {} | Route::Settings { .. } | Route::AddBooks {} | Route::CheckIn {}
        ),
    }
}

/// Renders the bottom navigation bar component.
#[component]
pub fn BottomNav() -> Element {
    let current = use_route::<Route>();
    let mut sheet_open = use_signal(|| false);
    rsx! {
        nav { class: "m-tabbar", aria_label: "Primary",
            MTab { to: Route::Landing {}, label: "Library", on: is_active(&current, TabKind::Library), glyph: tab_glyph_library() }
            MTab { to: Route::AuthorsIndex {}, label: "Authors", on: is_active(&current, TabKind::Authors), glyph: tab_glyph_authors() }
            button {
                r#type: "button",
                class: "m-tabbar-scan",
                "data-testid": "tabbar-scan",
                // Label avoids the substring "to" — the bottom nav renders in
                // the DOM on every web page (CSS-hidden on desktop), so a
                // "…to…" name collides with Playwright's getByLabel("To").
                aria_label: "Add books",
                aria_haspopup: "dialog",
                aria_expanded: if sheet_open() { "true" } else { "false" },
                onclick: move |_| sheet_open.set(true),
                span { class: "m-tabbar-scan-disc", {tab_glyph_scan()} }
                span { class: "m-tabbar-label", "Add" }
            }
            MTab { to: Route::Stats {}, label: "Stats", on: is_active(&current, TabKind::Stats), glyph: tab_glyph_stats() }
            MTab { to: Route::Account {}, label: "You", on: is_active(&current, TabKind::You), glyph: tab_glyph_you() }
        }
        AddBooksSheet { open: sheet_open, on_close: move |_| sheet_open.set(false) }
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

/// Glyph inside the raised center action — a plus, reading as "add" for the
/// Add-books sheet trigger.
fn tab_glyph_scan() -> Element {
    rsx! {
        svg {
            width: "24", height: "24", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            line { x1: "12", y1: "7", x2: "12", y2: "17" }
            line { x1: "7", y1: "12", x2: "17", y2: "12" }
        }
    }
}

fn tab_glyph_stats() -> Element {
    rsx! {
        svg {
            width: "22", height: "22", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M4 20V10" }
            path { d: "M10 20V4" }
            path { d: "M16 20v-6" }
            path { d: "M22 20H2" }
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
    fn authors_tab_lights_on_its_details() {
        assert!(is_active(&Route::AuthorsIndex {}, TabKind::Authors));
        assert!(is_active(&Route::AuthorDetail { id: 1 }, TabKind::Authors));
    }

    #[test]
    fn series_routes_light_no_tab_since_series_left_the_bar() {
        // Series is no longer a bottom tab; its routes must not light any tab.
        for tab in [
            TabKind::Library,
            TabKind::Authors,
            TabKind::Stats,
            TabKind::You,
        ] {
            assert!(!is_active(&Route::SeriesIndex {}, tab));
            assert!(!is_active(&Route::SeriesDetail { id: 1 }, tab));
        }
    }

    #[test]
    fn you_tab_lights_across_account_section() {
        assert!(is_active(&Route::Account {}, TabKind::You));
        assert!(is_active(&Route::Settings { section: None }, TabKind::You));
        assert!(is_active(&Route::AddBooks {}, TabKind::You));
        assert!(is_active(&Route::CheckIn {}, TabKind::You));
        assert!(!is_active(&Route::Landing {}, TabKind::You));
    }

    #[test]
    fn stats_tab_lights_only_on_the_stats_route() {
        assert!(is_active(&Route::Stats {}, TabKind::Stats));
        assert!(!is_active(&Route::Landing {}, TabKind::Stats));
        assert!(!is_active(&Route::Stats {}, TabKind::Library));
    }

    #[test]
    fn tabs_are_mutually_exclusive_on_landing() {
        let here = Route::Landing {};
        assert!(is_active(&here, TabKind::Library));
        assert!(!is_active(&here, TabKind::Authors));
        assert!(!is_active(&here, TabKind::Stats));
        assert!(!is_active(&here, TabKind::You));
    }

    #[test]
    fn account_route_lights_only_the_you_tab() {
        let here = Route::Account {};
        assert!(!is_active(&here, TabKind::Library));
        assert!(!is_active(&here, TabKind::Authors));
        assert!(!is_active(&here, TabKind::Stats));
        assert!(is_active(&here, TabKind::You));
    }
}
