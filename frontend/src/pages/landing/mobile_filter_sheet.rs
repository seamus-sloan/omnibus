//! Mobile "Sort & filter" bottom sheet for the library home screen.
//!
//! Writes straight into the shared [`ViewPrefs`] (each tap fires
//! `on_change`, which persists + refetches page 1 server-side), so the
//! footer's "Show N books" is just a close affordance over live results.

use dioxus::prelude::*;
use omnibus_shared::{SortDir, SortKey, ViewFilters, ViewPrefs};

/// Sort axes offered on mobile, in sheet order, with their row labels.
const SORT_ROWS: &[(SortKey, &str)] = &[
    (SortKey::NewestAdded, "Recently added"),
    (SortKey::LastUpdated, "Recently updated"),
    (SortKey::Title, "Title"),
    (SortKey::Author, "Author"),
    (SortKey::Series, "Series"),
];

/// Format chips: display label → the lowercase `ViewFilters::formats` values
/// the chip toggles. "Audiobook" covers every direct-play audio container.
const FORMAT_CHIPS: &[(&str, &[&str])] =
    &[("EPUB", &["epub"]), ("Audiobook", &["m4b", "m4a", "mp3"])];

/// Short label for the header's sort pill.
pub(super) fn sort_pill_label(key: SortKey) -> &'static str {
    match key {
        SortKey::NewestAdded => "Added",
        SortKey::LastUpdated => "Updated",
        SortKey::Title => "Title",
        SortKey::Author => "Author",
        SortKey::Series => "Series",
    }
}

/// Direction arrow for the header's sort pill.
pub(super) fn dir_arrow(dir: SortDir) -> &'static str {
    match dir {
        SortDir::Asc => "\u{2191}",
        SortDir::Desc => "\u{2193}",
    }
}

/// The direction the grid should default to when switching onto `key`:
/// newest-first for the date axes, ascending otherwise.
fn default_dir_for(key: SortKey) -> SortDir {
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

/// Human direction label shown on the selected sort row.
fn dir_label(key: SortKey, dir: SortDir) -> String {
    let text = match (key, dir) {
        (SortKey::NewestAdded | SortKey::LastUpdated, SortDir::Desc) => "Newest first",
        (SortKey::NewestAdded | SortKey::LastUpdated, SortDir::Asc) => "Oldest first",
        (_, SortDir::Asc) => "A\u{2013}Z",
        (_, SortDir::Desc) => "Z\u{2013}A",
    };
    format!("{text} {}", dir_arrow(dir))
}

/// Whether a format chip is active (its first wire value is filtered on).
fn chip_on(filters: &ViewFilters, values: &[&str]) -> bool {
    values
        .first()
        .is_some_and(|v| filters.formats.iter().any(|f| f == v))
}

/// Toggle a chip's format values in/out of the filter set.
fn toggle_formats(filters: &mut ViewFilters, values: &[&str]) {
    if chip_on(filters, values) {
        filters.formats.retain(|f| !values.contains(&f.as_str()));
    } else {
        for v in values {
            if !filters.formats.iter().any(|f| f == v) {
                filters.formats.push((*v).to_string());
            }
        }
    }
}

/// The bottom sheet. Fires `on_change` with a whole updated [`ViewPrefs`] on
/// every tap and `on_close` from the scrim / footer button.
#[component]
pub(super) fn MobileSortFilterSheet(
    prefs: ViewPrefs,
    book_count: usize,
    on_change: EventHandler<ViewPrefs>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let show_label = if book_count == 1 {
        "Show 1 book".to_string()
    } else {
        format!("Show {book_count} books")
    };
    let reset_prefs = prefs.clone();
    let on_reset = move |_| {
        let mut next = reset_prefs.clone();
        next.sort_key = SortKey::default();
        next.sort_dir = SortDir::default();
        next.filters = ViewFilters::default();
        on_change.call(next);
    };
    rsx! {
        div {
            class: "m-sheet-scrim",
            "data-testid": "mobile-sort-filter-sheet",
            onclick: move |e| on_close.call(e),
            div { class: "m-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "Sort & filter" }
                    button {
                        r#type: "button", class: "m-sheet-reset",
                        "data-testid": "mobile-filter-reset",
                        onclick: on_reset,
                        "Reset"
                    }
                }
                div { class: "m-sheet-body",
                    div { class: "label m-filter-label", "Sort by" }
                    div { class: "m-sort-list",
                        for &(key, label) in SORT_ROWS {
                            {sort_row(key, label, &prefs, &on_change)}
                        }
                    }
                    div { class: "label m-filter-label", "Format" }
                    div { class: "m-filter-chips",
                        {all_chip(&prefs, &on_change)}
                        for &(label, values) in FORMAT_CHIPS {
                            {format_chip(label, values, &prefs, &on_change)}
                        }
                    }
                }
                div { class: "m-sheet-foot",
                    button {
                        r#type: "button", class: "btn primary m-sheet-show",
                        "data-testid": "mobile-filter-show",
                        onclick: move |e| on_close.call(e),
                        "{show_label}"
                    }
                }
            }
        }
    }
}

/// One sort row: tapping a new axis selects it with its default direction;
/// tapping the selected axis flips direction.
fn sort_row(
    key: SortKey,
    label: &'static str,
    prefs: &ViewPrefs,
    on_change: &EventHandler<ViewPrefs>,
) -> Element {
    let on = prefs.sort_key == key;
    let dir = dir_label(key, prefs.sort_dir);
    let next_base = prefs.clone();
    let handler = *on_change;
    rsx! {
        button {
            key: "{label}",
            r#type: "button",
            class: if on { "m-sort-row on" } else { "m-sort-row" },
            onclick: move |_| {
                let mut next = next_base.clone();
                if next.sort_key == key {
                    next.sort_dir = match next.sort_dir {
                        SortDir::Asc => SortDir::Desc,
                        SortDir::Desc => SortDir::Asc,
                    };
                } else {
                    next.sort_key = key;
                    next.sort_dir = default_dir_for(key);
                }
                handler.call(next);
            },
            span { class: "m-sort-row-label", "{label}" }
            if on {
                span { class: "m-sort-row-dir",
                    "{dir}"
                    svg {
                        width: "15", height: "15", view_box: "0 0 15 15", fill: "none",
                        stroke: "currentColor", stroke_width: "1.8",
                        stroke_linecap: "round", stroke_linejoin: "round",
                        path { d: "M3 8l3 3 6-7" }
                    }
                }
            }
        }
    }
}

/// The "All" chip — active when no format filter is set; tapping clears them.
fn all_chip(prefs: &ViewPrefs, on_change: &EventHandler<ViewPrefs>) -> Element {
    let on = prefs.filters.formats.is_empty();
    let next_base = prefs.clone();
    let handler = *on_change;
    rsx! {
        button {
            r#type: "button",
            class: if on { "m-filter-chip on" } else { "m-filter-chip" },
            onclick: move |_| {
                let mut next = next_base.clone();
                next.filters.formats.clear();
                handler.call(next);
            },
            "All"
        }
    }
}

/// One format chip toggling its wire values.
fn format_chip(
    label: &'static str,
    values: &'static [&'static str],
    prefs: &ViewPrefs,
    on_change: &EventHandler<ViewPrefs>,
) -> Element {
    let on = chip_on(&prefs.filters, values);
    let next_base = prefs.clone();
    let handler = *on_change;
    rsx! {
        button {
            key: "{label}",
            r#type: "button",
            class: if on { "m-filter-chip on" } else { "m-filter-chip" },
            onclick: move |_| {
                let mut next = next_base.clone();
                toggle_formats(&mut next.filters, values);
                handler.call(next);
            },
            "{label}"
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_for_dates_is_desc_and_text_asc() {
        assert_eq!(default_dir_for(SortKey::NewestAdded), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::LastUpdated), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::Title), SortDir::Asc);
    }

    #[test]
    fn dir_label_reads_naturally_per_axis() {
        assert_eq!(
            dir_label(SortKey::NewestAdded, SortDir::Desc),
            "Newest first \u{2193}"
        );
        assert_eq!(
            dir_label(SortKey::Title, SortDir::Asc),
            "A\u{2013}Z \u{2191}"
        );
        assert_eq!(
            dir_label(SortKey::Author, SortDir::Desc),
            "Z\u{2013}A \u{2193}"
        );
    }

    #[test]
    fn toggle_formats_adds_and_removes_chip_groups() {
        let mut filters = ViewFilters::default();
        toggle_formats(&mut filters, &["m4b", "m4a", "mp3"]);
        assert_eq!(filters.formats, vec!["m4b", "m4a", "mp3"]);
        assert!(chip_on(&filters, &["m4b", "m4a", "mp3"]));
        // Adding a second group keeps the first.
        toggle_formats(&mut filters, &["epub"]);
        assert!(chip_on(&filters, &["epub"]));
        assert_eq!(filters.formats.len(), 4);
        // Toggling off removes only that group's values.
        toggle_formats(&mut filters, &["m4b", "m4a", "mp3"]);
        assert_eq!(filters.formats, vec!["epub"]);
    }
}
