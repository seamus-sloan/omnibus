use dioxus::prelude::*;
use omnibus_shared::{SortDir, SortKey, ViewMode, ViewPrefs};

use super::sorting::{
    default_dir_for, sort_key_from_value, sort_key_label, sort_key_value, toggle_dir, SORT_KEYS,
};

#[component]
pub(super) fn Toolbar(prefs: ViewPrefs, on_change: EventHandler<ViewPrefs>) -> Element {
    let view_mode = prefs.view_mode;
    let sort_key = prefs.sort_key;
    let sort_dir = prefs.sort_dir;
    let filters_open = prefs.filters_open;

    let apply = move |new_prefs: ViewPrefs| on_change.call(new_prefs);
    let set_view = {
        let prefs = prefs.clone();
        move |mode: ViewMode| {
            let mut next = prefs.clone();
            next.view_mode = mode;
            apply(next);
        }
    };
    let toggle_filters = {
        let prefs = prefs.clone();
        move |_| {
            let mut next = prefs.clone();
            next.filters_open = !next.filters_open;
            apply(next);
        }
    };
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
            apply(next);
        }
    };
    let toggle_sort_dir = {
        let prefs = prefs.clone();
        move |_| {
            let mut next = prefs.clone();
            next.sort_dir = toggle_dir(next.sort_dir);
            apply(next);
        }
    };

    let set_view_table = set_view.clone();
    let set_view_grid = set_view.clone();

    rsx! {
        div { class: "lib-toolbar", role: "toolbar", "data-testid": "lib-toolbar",
            button {
                class: "lib-toggle-btn lib-filters-btn",
                "aria-pressed": "{filters_open}",
                "data-testid": "lib-filters-toggle",
                aria_label: "Toggle filter sidebar",
                onclick: toggle_filters,
                "Filters"
            }
            // Pressed-button toggle group, not an ARIA tablist — there are no
            // associated tab panels and no arrow-key tab navigation, so
            // `aria-pressed` on plain `<button>`s is the right shape.
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

            if view_mode == ViewMode::Grid {
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
    }
}
