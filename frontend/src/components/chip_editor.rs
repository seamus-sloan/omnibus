//! Generic add/remove chip editor with a substring-match suggestion dropdown.
//!
//! Used by the F5.1 metadata edit page for authors and tags, and by the
//! F5.9-lite landing-table inline editor for the Authors column. The
//! component renders as a sequence of chip elements followed by an
//! input + optional dropdown — the consumer wraps the output in
//! whatever flex container fits their layout (`me-chip-row`,
//! `me-tag-chips`, ...).
//!
//! The dropdown surfaces up to [`MAX_SUGGESTIONS`] case-insensitive
//! substring matches against `suggestions`, excluding values already
//! present in `values`. Keyboard: ↓/↑ moves the highlight, Enter
//! commits (highlighted suggestion when present, raw input otherwise),
//! Escape clears the highlight. Pass an empty signal to disable the
//! dropdown entirely and fall back to plain free-text entry.
//!
//! Suggestions are passed in as a [`ReadSignal`] so the component
//! reads them by reference each render instead of taking ownership of a
//! cloned `Vec` on every keystroke. The consumer owns the candidate pool
//! — usually derived from a fetched list (`data::list_authors`,
//! `data::get_tag_cloud`) or a flat-uniq of an already-loaded book list.

use dioxus::prelude::*;

/// Cap on suggestion rows. Five fits without scroll on a typical
/// edit-row layout and matches the explicit "< 5 relevant suggestions"
/// user requirement from the F5.9-lite plan.
const MAX_SUGGESTIONS: usize = 5;

#[derive(Props, PartialEq, Clone)]
pub struct ChipEditorProps {
    /// The chip list. Shared with the consumer so the parent can read
    /// it for dirty-detection / persistence.
    pub values: Signal<Vec<String>>,
    /// Placeholder shown inside the chip-input.
    pub placeholder: String,
    /// Fired after every chip add or remove with the new full list, so
    /// the consumer can persist on change (e.g. POST overrides) without
    /// having to re-subscribe to the signal.
    pub on_change: EventHandler<Vec<String>>,
    /// Candidate pool, read by reference each render via the wrapping
    /// `ReadSignal`. An empty signal suppresses the dropdown
    /// entirely. Callers usually own a `Signal<Vec<String>>` and pass
    /// it directly — `From<Signal<T>>` does the wrap.
    pub suggestions: ReadSignal<Vec<String>>,
    /// When true, each chip is prefixed with an uppercase-initials
    /// avatar. Used by author chips (`me-avatar`); off for tags.
    #[props(default = false)]
    pub show_avatar: bool,
    /// CSS class for the inline `<input>`. Defaults to `me-chip-input`
    /// (the author/landing-row style); tag rows pass `me-tag-input`.
    #[props(default = "me-chip-input".to_string())]
    pub input_class: String,
    /// Prefix for the per-chip Remove button's `aria-label`, e.g.
    /// "Remove" → "Remove Ada Lovelace" or "Remove tag" → "Remove tag
    /// fiction".
    #[props(default = "Remove".to_string())]
    pub aria_remove_prefix: String,
    /// Per-instance testid prefix so multiple ChipEditors on one page
    /// don't collide. The `<input>` gets `<prefix>-input` and the
    /// suggestions `<ul>` gets `<prefix>-suggestions`. Suggestion
    /// `<li>`s use `role="option"` with the suggestion as accessible
    /// name (no per-row testid) so Playwright resolves them via
    /// `getByRole("option", { name })`.
    #[props(default = "chip-editor".to_string())]
    pub testid_prefix: String,
}

#[component]
pub fn ChipEditor(props: ChipEditorProps) -> Element {
    let mut input = use_signal(String::new);
    let mut highlight = use_signal::<Option<usize>>(|| None);

    // Filter on every render. Reads the suggestion pool via the
    // ReadSignal so we never clone the underlying Vec — only the
    // ≤5 matches that actually make it into the dropdown get cloned
    // out.
    let filtered: Vec<String> = {
        let query = input().trim().to_lowercase();
        let suggestions = props.suggestions.read();
        if query.is_empty() || suggestions.is_empty() {
            Vec::new()
        } else {
            // Normalize both sides with `to_lowercase()` (Unicode-aware)
            // so the dedup check matches what we use in `commit()`. The
            // old `eq_ignore_ascii_case` path could let non-ASCII
            // variants ("Maas"/"Máas") slip past the dedup.
            let current: std::collections::HashSet<String> = props
                .values
                .read()
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            suggestions
                .iter()
                .filter(|s| {
                    let lc = s.to_lowercase();
                    lc.contains(&query) && !current.contains(&lc)
                })
                .take(MAX_SUGGESTIONS)
                .cloned()
                .collect()
        }
    };

    let filtered_for_keydown = filtered.clone();
    let mut values_sig = props.values;
    let on_change = props.on_change;
    let mut commit = move |name: String| {
        let trimmed = name.trim().to_string();
        if trimmed.is_empty() {
            return;
        }
        // Dedup uses the same Unicode-aware `to_lowercase()` comparison
        // the filter loop above runs against the suggestion pool, so a
        // dropdown-surfaced duplicate and a duplicate typed-in by hand
        // are treated identically (the old `eq_ignore_ascii_case` would
        // let through case-variant duplicates that contain non-ASCII).
        let trimmed_lc = trimmed.to_lowercase();
        if values_sig
            .read()
            .iter()
            .any(|v| v.to_lowercase() == trimmed_lc)
        {
            input.set(String::new());
            highlight.set(None);
            return;
        }
        let mut new_values = values_sig.read().clone();
        new_values.push(trimmed);
        values_sig.set(new_values.clone());
        on_change.call(new_values);
        input.set(String::new());
        highlight.set(None);
    };

    let testid_suggestions = format!("{}-suggestions", props.testid_prefix);
    let testid_input = format!("{}-input", props.testid_prefix);

    rsx! {
        Fragment {
            for (i, value) in props.values.read().iter().cloned().enumerate() {
                div {
                    class: "chip me-chip-item",
                    key: "{i}-{value}",
                    if props.show_avatar {
                        span { class: "me-avatar",
                            {value.chars().filter(|c| c.is_uppercase()).take(2).collect::<String>()}
                        }
                    }
                    "{value}"
                    button {
                        class: "me-chip-remove",
                        "aria-label": "{props.aria_remove_prefix} {value}",
                        onclick: move |_| {
                            let mut new_values = values_sig.read().clone();
                            if i < new_values.len() {
                                new_values.remove(i);
                                values_sig.set(new_values.clone());
                                on_change.call(new_values);
                            }
                        },
                        "\u{2715}"
                    }
                }
            }
            div { class: "chip-editor-input-wrap",
                input {
                    class: "{props.input_class}",
                    "data-testid": "{testid_input}",
                    placeholder: "{props.placeholder}",
                    value: "{input}",
                    oninput: move |e| {
                        input.set(e.value());
                        highlight.set(None);
                    },
                    onkeydown: move |e| {
                        match e.key() {
                            Key::Enter => {
                                e.prevent_default();
                                let typed = input();
                                let value = match highlight() {
                                    Some(idx) => filtered_for_keydown
                                        .get(idx)
                                        .cloned()
                                        .unwrap_or(typed),
                                    None => typed,
                                };
                                commit(value);
                            }
                            Key::ArrowDown if !filtered_for_keydown.is_empty() => {
                                e.prevent_default();
                                let next = match highlight() {
                                    Some(i) if i + 1 < filtered_for_keydown.len() => Some(i + 1),
                                    _ => Some(0),
                                };
                                highlight.set(next);
                            }
                            Key::ArrowUp if !filtered_for_keydown.is_empty() => {
                                e.prevent_default();
                                let next = match highlight() {
                                    Some(0) | None => Some(filtered_for_keydown.len() - 1),
                                    Some(i) => Some(i - 1),
                                };
                                highlight.set(next);
                            }
                            Key::Escape => {
                                highlight.set(None);
                            }
                            _ => {}
                        }
                    },
                }
                if !filtered.is_empty() {
                    ul {
                        class: "chip-editor-suggestions",
                        role: "listbox",
                        "data-testid": "{testid_suggestions}",
                        for (i, suggestion) in filtered.iter().cloned().enumerate() {
                            li {
                                key: "{i}-{suggestion}",
                                class: if Some(i) == highlight() {
                                    "chip-editor-suggestion is-active"
                                } else {
                                    "chip-editor-suggestion"
                                },
                                role: "option",
                                aria_selected: if Some(i) == highlight() { "true" } else { "false" },
                                // mousedown (not click) fires before the
                                // input's blur, so the input keeps focus
                                // for the next add.
                                onmousedown: {
                                    let suggestion = suggestion.clone();
                                    move |e: Event<MouseData>| {
                                        e.prevent_default();
                                        commit(suggestion.clone());
                                    }
                                },
                                "{suggestion}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Helper: flat-uniq a slice of `Vec<String>`-shaped iterables into a
/// sorted candidate pool suitable for [`ChipEditorProps::suggestions`].
/// Case-insensitive dedup (keeping the first-seen casing) so a library
/// with mixed `Sarah J. Maas` / `sarah j. maas` collapses to one row.
pub fn collect_suggestions<'a, I>(sources: I) -> Vec<String>
where
    I: IntoIterator<Item = &'a String>,
{
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<String, String> = BTreeMap::new();
    for s in sources {
        let key = s.to_lowercase();
        seen.entry(key).or_insert_with(|| s.clone());
    }
    seen.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_suggestions_dedups_case_insensitively() {
        let a = "Sarah J. Maas".to_string();
        let b = "sarah j. maas".to_string();
        let c = "Brandon Sanderson".to_string();
        let out = collect_suggestions([&a, &b, &c]);
        assert_eq!(out.len(), 2, "case-variant duplicates must collapse");
        assert!(out.iter().any(|s| s == "Brandon Sanderson"));
        // First-seen casing wins.
        assert!(out.iter().any(|s| s == "Sarah J. Maas"));
    }

    #[test]
    fn collect_suggestions_returns_sorted_output() {
        let a = "Zelda".to_string();
        let b = "Ada".to_string();
        let c = "Mira".to_string();
        let out = collect_suggestions([&a, &b, &c]);
        // BTreeMap iterates by lowercased key, so output is alphabetic
        // case-insensitive.
        assert_eq!(out, vec!["Ada", "Mira", "Zelda"]);
    }
}
