//! Toolbar (view mode + sort key + sort direction) for the landing page.
//!
//! Stateless: emits a new [`ViewPrefs`] through the parent's `on_change`
//! handler so [`super::LandingPage`] owns the canonical signal.

use dioxus::prelude::*;
use omnibus_shared::{SortDir, SortKey, ViewMode, ViewPrefs};

use super::sorting::{
    default_dir_for, sort_key_from_value, sort_key_label, sort_key_value, toggle_dir, SORT_KEYS,
};

#[component]
pub(super) fn Toolbar(prefs: ViewPrefs, on_change: EventHandler<ViewPrefs>) -> Element {
    let view_mode = prefs.view_mode;

    rsx! {
        div { class: "lib-toolbar", role: "toolbar", "data-testid": "lib-toolbar",
            ViewModeToggle { view_mode, prefs: prefs.clone(), on_change }
            if view_mode == ViewMode::Grid {
                SortControls { prefs, on_change }
            }
        }
    }
}

/// Table/Grid pressed-button toggle group. Not an ARIA tablist — there are no
/// associated tab panels and no arrow-key tab navigation, so `aria-pressed`
/// on plain `<button>`s is the right shape.
#[component]
fn ViewModeToggle(
    view_mode: ViewMode,
    prefs: ViewPrefs,
    on_change: EventHandler<ViewPrefs>,
) -> Element {
    let set_view = move |mode: ViewMode| {
        let mut next = prefs.clone();
        next.view_mode = mode;
        on_change.call(next);
    };
    let set_view_table = set_view.clone();
    let set_view_grid = set_view.clone();

    rsx! {
        div { class: "lib-view-toggle", "aria-label": "View mode",
            button {
                class: "lib-toggle-btn",
                "aria-pressed": "{view_mode == ViewMode::Table}",
                "data-testid": "view-toggle-table",
                onclick: move |_| set_view_table(ViewMode::Table),
                "Table"
            }
            button {
                class: "lib-toggle-btn",
                "aria-pressed": "{view_mode == ViewMode::Grid}",
                "data-testid": "view-toggle-grid",
                onclick: move |_| set_view_grid(ViewMode::Grid),
                "Grid"
            }
        }
    }
}

/// Grid-only sort-axis dropdown and direction toggle.
#[component]
fn SortControls(prefs: ViewPrefs, on_change: EventHandler<ViewPrefs>) -> Element {
    let sort_key = prefs.sort_key;
    let sort_dir = prefs.sort_dir;
    let set_sort_key = {
        let prefs = prefs.clone();
        move |key: SortKey| {
            let mut next = prefs.clone();
            // Switching to a different axis from the grid dropdown should
            // adopt that axis's natural direction (descending for time-based
            // axes, ascending for alphabetical) — matches the table-view
            // header behavior so the two views stay consistent.
            if next.sort_key != key {
                next.sort_dir = default_dir_for(key);
            }
            next.sort_key = key;
            on_change.call(next);
        }
    };
    let toggle_sort_dir = move |_| {
        let mut next = prefs.clone();
        next.sort_dir = toggle_dir(next.sort_dir);
        on_change.call(next);
    };

    rsx! {
        div { class: "lib-sort-controls",
            label { class: "lib-sort-label",
                "Sort by"
                select {
                    class: "lib-sort-select",
                    "data-testid": "lib-sort-select",
                    onchange: move |evt: Event<FormData>| {
                        if let Some(key) = sort_key_from_value(&evt.value()) {
                            set_sort_key(key);
                        }
                    },
                    for opt in SORT_KEYS.iter().copied() {
                        option {
                            key: "{sort_key_value(opt)}",
                            value: "{sort_key_value(opt)}",
                            selected: opt == sort_key,
                            "{sort_key_label(opt)}"
                        }
                    }
                }
            }
            button {
                class: "lib-sort-dir",
                "data-testid": "lib-sort-dir",
                aria_label: "Toggle sort direction",
                onclick: toggle_sort_dir,
                if sort_dir == SortDir::Asc { "↑" } else { "↓" }
            }
        }
    }
}

// SSR render-smoke coverage — `render_element` supplies a runtime, but the
// `on_change` EventHandler must be built inside a component body, so the tests
// mount the toolbar through a tiny prop-only harness.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render;

    #[component]
    fn ToolbarHarness(prefs: ViewPrefs) -> Element {
        rsx! {
            Toolbar { prefs, on_change: move |_| {} }
        }
    }

    fn render_toolbar(prefs: ViewPrefs) -> String {
        render(rsx! {
            ToolbarHarness { prefs }
        })
    }

    #[test]
    fn toolbar_renders_the_view_toggle_with_grid_pressed_by_default() {
        let html = render_toolbar(ViewPrefs::default());

        assert!(html.contains("data-testid=\"lib-toolbar\""));
        assert!(html.contains("role=\"toolbar\""));
        assert!(html.contains("data-testid=\"view-toggle-table\""));
        assert!(html.contains("data-testid=\"view-toggle-grid\""));
        assert!(html.contains("Table"));
        assert!(html.contains("Grid"));
        // Default view mode is Grid, so its button is the pressed one and the
        // grid-only sort controls are part of the default markup.
        assert!(html.contains("aria-pressed=\"true\""));
        assert!(html.contains("data-testid=\"lib-sort-select\""));
        assert!(html.contains("data-testid=\"lib-sort-dir\""));
        assert!(html.contains("Sort by"));
    }

    #[test]
    fn toolbar_hides_the_sort_controls_in_table_mode() {
        let prefs = ViewPrefs {
            view_mode: ViewMode::Table,
            ..ViewPrefs::default()
        };
        let html = render_toolbar(prefs);

        // Table view sorts via its own column headers, so the grid-only sort
        // dropdown stays out of the markup.
        assert!(!html.contains("data-testid=\"lib-sort-select\""));
        assert!(!html.contains("data-testid=\"lib-sort-dir\""));
    }
}
