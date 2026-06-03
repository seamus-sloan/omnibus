//! Filter-sidebar UI components for the landing page.
//!
//! Renders the author / series / format facet checklist + format chips
//! alongside an "empty after filtering" placeholder. The parent
//! [`super::LandingPage`] owns the [`ViewFilters`] signal these emit into.

use std::collections::HashSet;

use dioxus::prelude::*;
use omnibus_shared::ViewFilters;

use super::filtering::{format_display_label, FacetCounts};

#[component]
pub(super) fn FilterSidebar(
    facets: FacetCounts,
    filters: ViewFilters,
    on_change: EventHandler<ViewFilters>,
) -> Element {
    // Reflect every filter bucket — including `formats` set via the top
    // chip row — so a user with only a format chip selected still sees
    // the sidebar's "Clear filters" affordance.
    let any_active = !filters.is_empty();
    let toggle = {
        let filters = filters.clone();
        move |group: &'static str, value: String| {
            let mut next = filters.clone();
            let bucket = match group {
                "authors" => &mut next.authors,
                "series" => &mut next.series,
                "tags" => &mut next.tags,
                other => {
                    debug_assert!(
                        false,
                        "FilterSidebar::toggle: unknown facet group {other:?}"
                    );
                    return;
                }
            };
            if let Some(pos) = bucket.iter().position(|v| v == &value) {
                bucket.remove(pos);
            } else {
                bucket.push(value);
            }
            on_change.call(next);
        }
    };

    rsx! {
        aside { class: "lib-sidebar", "data-testid": "lib-sidebar", aria_label: "Filters",
            if any_active {
                button {
                    class: "lib-clear-filters",
                    "data-testid": "lib-clear-filters",
                    onclick: move |_| on_change.call(ViewFilters::default()),
                    "Clear filters"
                }
            }

            FacetSection {
                title: "Authors",
                testid: "lib-facet-authors",
                items: facets.authors.clone(),
                selected: filters.authors.clone(),
                on_toggle: {
                    let toggle = toggle.clone();
                    move |v: String| toggle("authors", v)
                },
            }
            FacetSection {
                title: "Series",
                testid: "lib-facet-series",
                items: facets.series.clone(),
                selected: filters.series.clone(),
                on_toggle: {
                    let toggle = toggle.clone();
                    move |v: String| toggle("series", v)
                },
            }
            FacetSection {
                title: "Tags",
                testid: "lib-facet-tags",
                items: facets.tags.clone(),
                selected: filters.tags.clone(),
                on_toggle: {
                    let toggle = toggle.clone();
                    move |v: String| toggle("tags", v)
                },
            }
        }
    }
}

#[component]
fn FacetSection(
    title: String,
    testid: String,
    items: Vec<(String, usize)>,
    selected: Vec<String>,
    on_toggle: EventHandler<String>,
) -> Element {
    if items.is_empty() {
        return rsx! { Fragment {} };
    }
    let selected_set: HashSet<&String> = selected.iter().collect();
    rsx! {
        section { class: "lib-facet", "data-testid": "{testid}",
            h3 { class: "lib-facet-title", "{title}" }
            ul { class: "lib-chip-list",
                for (name, count) in items.iter() {
                    li {
                        key: "{name}",
                        button {
                            // Layer Atrium's `.chip` look onto the existing
                            // `.lib-chip` class — the Playwright selector
                            // `button.lib-chip[data-value="…"]` still matches.
                            class: "chip lib-chip",
                            "aria-pressed": "{selected_set.contains(&name)}",
                            "data-value": "{name}",
                            title: "{name}",
                            onclick: {
                                let name = name.clone();
                                move |_| on_toggle.call(name.clone())
                            },
                            span { class: "lib-chip-label", "{name}" }
                            span { class: "count lib-chip-count", "{count}" }
                        }
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Format chips (top-of-page inline filter)
// ---------------------------------------------------------------------------

#[component]
pub(super) fn FormatChips(
    counts: Vec<(String, usize)>,
    visible_count: usize,
    book_count: usize,
    selected: Vec<String>,
    on_change: EventHandler<Vec<String>>,
) -> Element {
    if counts.is_empty() {
        return rsx! { Fragment {} };
    }
    let selected_set: HashSet<String> = selected.iter().cloned().collect();
    let all_active = selected.is_empty();

    rsx! {
        div { class: "lib-format-chips",
            "data-testid": "lib-format-chips",
            role: "group",
            aria_label: "Format filters",

            span { class: "label lib-format-chips-label", "Filter" }

            button {
                class: if all_active { "chip on" } else { "chip" },
                "data-format": "all",
                "aria-pressed": "{all_active}",
                onclick: move |_| on_change.call(Vec::new()),
                // Count distinct books, not per-format memberships — a book
                // with EPUB+M4B contributes 1, not 2.
                "All formats "
                span { class: "count", "{book_count}" }
            }

            for (key, count) in counts.into_iter() {
                {
                    let is_selected = selected_set.contains(&key);
                    let label = format_display_label(&key);
                    let key_for_click = key.clone();
                    let selected_for_click = selected.clone();
                    rsx! {
                        button {
                            key: "{key}",
                            class: if is_selected { "chip on" } else { "chip" },
                            "data-format": "{key}",
                            "aria-pressed": "{is_selected}",
                            onclick: move |_| {
                                let mut next: Vec<String> = selected_for_click.clone();
                                if let Some(pos) = next.iter().position(|v| v == &key_for_click) {
                                    next.remove(pos);
                                } else {
                                    next.push(key_for_click.clone());
                                }
                                on_change.call(next);
                            },
                            "{label} "
                            span { class: "count", "{count}" }
                        }
                    }
                }
            }

            div { class: "lib-format-chips-spacer" }
            span { class: "mono lib-format-chips-count",
                "{visible_count} of {book_count}"
            }
        }
    }
}

#[component]
pub(super) fn EmptyFiltered(on_clear: EventHandler<()>) -> Element {
    rsx! {
        div { class: "library-empty",
            p { "No books match these filters." }
            button {
                class: "btn",
                "data-testid": "lib-clear-filters-empty",
                onclick: move |_| on_clear.call(()),
                "Clear filters"
            }
        }
    }
}
