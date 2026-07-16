//! Shared autocomplete-dropdown machinery used by both the multi-value
//! [`super::chip_editor::ChipEditor`] and the single-value
//! [`super::suggest_field::SuggestField`]: the per-render selection state,
//! the dropdown component itself, and the substring-match filter that
//! produces it.

use dioxus::prelude::*;

use super::chip_editor::SuggestionItem;

#[cfg(test)]
mod tests;

/// Cap on suggestion rows. Five fits without scroll on a typical
/// edit-row layout and matches the "< 5 relevant suggestions"
/// user requirement.
const MAX_SUGGESTIONS: usize = 5;

/// Per-render selection state for the dropdown: the visible suggestion
/// rows, whether a "+ Create" trailing row is shown, the raw text to
/// quote inside that row, and the keyboard highlight cursor. Grouped
/// so the dropdown signature stays compact as new pieces of selection
/// state accrete.
#[derive(Clone, PartialEq)]
pub(super) struct DropdownSelectionState {
    pub(super) filtered: Vec<SuggestionItem>,
    pub(super) show_create_row: bool,
    pub(super) typed: String,
    pub(super) highlight: Option<usize>,
}

/// Autocomplete dropdown — filtered suggestion rows plus an optional
/// "+ Create" trailing row. `on_pick` fires with the chosen name.
#[component]
pub(super) fn SuggestionDropdown(
    selection: DropdownSelectionState,
    dropdown_header: String,
    testid: String,
    on_pick: EventHandler<String>,
) -> Element {
    let DropdownSelectionState {
        filtered,
        show_create_row,
        typed,
        highlight,
    } = selection;
    let create_row_index = filtered.len();
    rsx! {
        ul {
            class: "chip-editor-suggestions",
            role: "listbox",
            "data-testid": "{testid}",
            if !dropdown_header.is_empty() {
                li {
                    class: "chip-editor-suggestions-header",
                    aria_hidden: "true",
                    "{dropdown_header}"
                }
            }
            for (i, item) in filtered.iter().cloned().enumerate() {
                li {
                    key: "{i}-{item.name}",
                    // Single-line if/else avoids nightly
                    // clippy's `suspicious_else_formatting`
                    // lint, which trips on the multi-line
                    // form inside rsx attribute slots.
                    class: if Some(i) == highlight { "chip-editor-suggestion is-active" } else { "chip-editor-suggestion" },
                    role: "option",
                    aria_selected: if Some(i) == highlight { "true" } else { "false" },
                    // mousedown (not click) fires before the
                    // input's blur, so the input keeps focus
                    // for the next add.
                    onmousedown: {
                        let name = item.name.clone();
                        move |e: Event<MouseData>| {
                            e.prevent_default();
                            on_pick.call(name.clone());
                        }
                    },
                    span { class: "chip-editor-suggestion-name", "{item.name}" }
                    span { class: "chip-editor-suggestion-count",
                        if item.count == 1 { "1 book" } else { "{item.count} books" }
                    }
                    span { class: "chip-editor-suggestion-enter", aria_hidden: "true", "\u{21a9}" }
                }
            }
            if show_create_row {
                li {
                    key: "__create__-{typed}",
                    class: if Some(create_row_index) == highlight { "chip-editor-suggestion chip-editor-suggestion--create is-active" } else { "chip-editor-suggestion chip-editor-suggestion--create" },
                    role: "option",
                    aria_selected: if Some(create_row_index) == highlight { "true" } else { "false" },
                    onmousedown: {
                        let value = typed.clone();
                        move |e: Event<MouseData>| {
                            e.prevent_default();
                            on_pick.call(value.clone());
                        }
                    },
                    span { class: "chip-editor-suggestion-name",
                        "+ Create "
                        span { class: "chip-editor-suggestion-quote", "\"{typed}\"" }
                    }
                    span { class: "chip-editor-suggestion-enter", aria_hidden: "true", "\u{21a9}" }
                }
            }
        }
    }
}

/// Compute the ≤`MAX_SUGGESTIONS` autocomplete candidates for the current
/// query and focus state. Returns an empty `Vec` when the pool is empty,
/// when the query doesn't match anything, or when the input is unfocused
/// and the query is empty (open-on-focus is suppressed by `suppress_open`).
///
/// Filtering uses Unicode-aware `to_lowercase()` so ASCII case variants and
/// non-ASCII look-alikes don't slip past the dedup check.
pub(super) fn compute_suggestions(
    suggestions: &[SuggestionItem],
    current_values: &[String],
    query_lc: &str,
    focused: bool,
    suppress_open: bool,
) -> Vec<SuggestionItem> {
    if suggestions.is_empty() {
        return Vec::new();
    }
    let current: std::collections::HashSet<String> =
        current_values.iter().map(|s| s.to_lowercase()).collect();
    if query_lc.is_empty() {
        if focused && !suppress_open {
            suggestions
                .iter()
                .filter(|item| !current.contains(&item.name.to_lowercase()))
                .take(MAX_SUGGESTIONS)
                .cloned()
                .collect()
        } else {
            Vec::new()
        }
    } else {
        suggestions
            .iter()
            .filter(|item| {
                let lc = item.name.to_lowercase();
                lc.contains(query_lc) && !current.contains(&lc)
            })
            .take(MAX_SUGGESTIONS)
            .cloned()
            .collect()
    }
}
