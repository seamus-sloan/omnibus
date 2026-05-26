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

/// One entry in the autocomplete pool. Carries the canonical name plus
/// the number of books currently linked to it, both of which the
/// dropdown row renders. Counts are display-only — the component never
/// branches on them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SuggestionItem {
    pub name: String,
    pub count: usize,
}

impl SuggestionItem {
    pub fn new(name: impl Into<String>, count: usize) -> Self {
        Self {
            name: name.into(),
            count,
        }
    }
}

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
    /// Candidate pool. Each item carries the canonical name plus the
    /// number of books currently linked to it, which the dropdown
    /// renders to the right of the name as a quiet "1 book" / "23
    /// books" hint (so admins can tell apart "Min Jin Lee" with 1
    /// book from "Min Kym" with 0). Read by reference each render via
    /// the wrapping `ReadSignal`; an empty signal suppresses the
    /// dropdown entirely.
    pub suggestions: ReadSignal<Vec<SuggestionItem>>,
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
    /// When `true`, the inline `<input>` receives `autofocus` so the
    /// dropdown surfaces on first paint. Used by the landing-page
    /// Authors cell so admins don't have to click twice (once to
    /// enter edit mode, once to focus the input). Off by default to
    /// keep the F5.1 metadata-edit page from stealing focus from the
    /// page's other fields on mount.
    #[props(default = false)]
    pub autofocus: bool,
    /// Optional uppercase mini-header rendered at the top of the
    /// suggestion dropdown — "ADD AUTHOR" / "ADD TAG" in the F5.9-lite
    /// design comp. Empty string (the default) suppresses the header
    /// entirely.
    #[props(default)]
    pub dropdown_header: String,
    /// Fired when the user presses Escape inside the input. Useful for
    /// host components that want to exit a wrapping edit mode in
    /// addition to clearing the dropdown highlight. Default no-op.
    #[props(default)]
    pub on_close: EventHandler<()>,
}

#[component]
pub fn ChipEditor(props: ChipEditorProps) -> Element {
    let mut input = use_signal(String::new);
    let mut highlight = use_signal::<Option<usize>>(|| None);
    // Focus state drives the "open-on-focus" dropdown behavior the
    // design comp asks for: clicking into the input surfaces the top
    // `MAX_SUGGESTIONS` candidates immediately so the admin can pick
    // by sight, without having to type a character first to "wake up"
    // the autocomplete. `onblur` clears it (and the highlight) so the
    // dropdown collapses when focus leaves.
    let mut focused = use_signal(|| false);

    // Filter on every render. Reads the suggestion pool by reference
    // so we never clone the underlying Vec — only the ≤5 matches that
    // actually make it into the dropdown get cloned out.
    //
    // When `query_lc` is empty AND the input is focused, we surface
    // the first `MAX_SUGGESTIONS` not-already-chosen items so the
    // dropdown is useful on first focus. When `query_lc` is non-empty
    // we substring-filter as before.
    let query_lc = input().trim().to_lowercase();
    let filtered: Vec<SuggestionItem> = {
        let suggestions = props.suggestions.read();
        if suggestions.is_empty() {
            Vec::new()
        } else {
            // Normalize via `to_lowercase()` (Unicode-aware) so the
            // dedup-against-existing-values check matches what
            // `commit()` uses. The old `eq_ignore_ascii_case` path
            // could let non-ASCII variants ("Maas"/"Máas") slip past.
            let current: std::collections::HashSet<String> = props
                .values
                .read()
                .iter()
                .map(|s| s.to_lowercase())
                .collect();
            if query_lc.is_empty() {
                if focused() {
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
                        lc.contains(&query_lc) && !current.contains(&lc)
                    })
                    .take(MAX_SUGGESTIONS)
                    .cloned()
                    .collect()
            }
        }
    };

    // Render the "+ Create '<query>'" footer row when the user has typed
    // something but no surviving suggestion is an exact (case-
    // insensitive) match. This is the Tom-Yorke-style affordance: type
    // a brand-new name, hit Enter or click the row, and the chip is
    // added without you having to dismiss the dropdown first.
    let typed = input().trim().to_string();
    let show_create_row = !typed.is_empty()
        && !filtered
            .iter()
            .any(|item| item.name.to_lowercase() == query_lc)
        && !props
            .values
            .read()
            .iter()
            .any(|v| v.to_lowercase() == query_lc);

    // Total selectable rows in the dropdown: filtered suggestions plus
    // an optional "+ Create" trailing row. The create row, when shown,
    // is at index `filtered.len()`. ↑/↓ navigation wraps across the
    // whole set so the admin can land on Create with one keystroke
    // after a typed-from-scratch entry.
    let filtered_for_keydown = filtered.clone();
    let create_value_for_keydown = typed.clone();
    let create_row_index = filtered_for_keydown.len();
    let total_rows = filtered_for_keydown.len() + usize::from(show_create_row);

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
                    autofocus: props.autofocus,
                    onfocus: move |_| {
                        focused.set(true);
                    },
                    onblur: move |_| {
                        focused.set(false);
                        highlight.set(None);
                    },
                    oninput: move |e| {
                        input.set(e.value());
                        highlight.set(None);
                    },
                    onkeydown: move |e| {
                        match e.key() {
                            Key::Enter => {
                                e.prevent_default();
                                let typed_now = input();
                                // Dispatch to highlighted row (suggestion or
                                // create) when present; otherwise commit the
                                // raw typed value.
                                let value = match highlight() {
                                    Some(idx) if idx < filtered_for_keydown.len() => {
                                        filtered_for_keydown
                                            .get(idx)
                                            .map(|s| s.name.clone())
                                            .unwrap_or(typed_now)
                                    }
                                    Some(idx) if idx == create_row_index && show_create_row => {
                                        create_value_for_keydown.clone()
                                    }
                                    _ => typed_now,
                                };
                                commit(value);
                            }
                            Key::ArrowDown if total_rows > 0 => {
                                e.prevent_default();
                                let next = match highlight() {
                                    Some(i) if i + 1 < total_rows => Some(i + 1),
                                    _ => Some(0),
                                };
                                highlight.set(next);
                            }
                            Key::ArrowUp if total_rows > 0 => {
                                e.prevent_default();
                                let next = match highlight() {
                                    Some(0) | None => Some(total_rows - 1),
                                    Some(i) => Some(i - 1),
                                };
                                highlight.set(next);
                            }
                            Key::Escape => {
                                highlight.set(None);
                                props.on_close.call(());
                            }
                            _ => {}
                        }
                    },
                }
                if !filtered.is_empty() || show_create_row {
                    ul {
                        class: "chip-editor-suggestions",
                        role: "listbox",
                        "data-testid": "{testid_suggestions}",
                        if !props.dropdown_header.is_empty() {
                            li {
                                class: "chip-editor-suggestions-header",
                                aria_hidden: "true",
                                "{props.dropdown_header}"
                            }
                        }
                        for (i, item) in filtered.iter().cloned().enumerate() {
                            li {
                                key: "{i}-{item.name}",
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
                                    let name = item.name.clone();
                                    move |e: Event<MouseData>| {
                                        e.prevent_default();
                                        commit(name.clone());
                                    }
                                },
                                span { class: "chip-editor-suggestion-name", "{item.name}" }
                                span { class: "chip-editor-suggestion-count",
                                    if item.count == 1 { "1 book" } else { "{item.count} books" }
                                }
                                span { class: "chip-editor-suggestion-enter",
                                    aria_hidden: "true",
                                    "\u{21a9}"
                                }
                            }
                        }
                        if show_create_row {
                            li {
                                key: "__create__-{typed}",
                                class: if Some(create_row_index) == highlight() {
                                    "chip-editor-suggestion chip-editor-suggestion--create is-active"
                                } else {
                                    "chip-editor-suggestion chip-editor-suggestion--create"
                                },
                                role: "option",
                                aria_selected: if Some(create_row_index) == highlight() { "true" } else { "false" },
                                onmousedown: {
                                    let value = typed.clone();
                                    move |e: Event<MouseData>| {
                                        e.prevent_default();
                                        commit(value.clone());
                                    }
                                },
                                span { class: "chip-editor-suggestion-name",
                                    "+ Create "
                                    span { class: "chip-editor-suggestion-quote", "\"{typed}\"" }
                                }
                                span { class: "chip-editor-suggestion-enter",
                                    aria_hidden: "true",
                                    "\u{21a9}"
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Helper: dedup a list of `(name, count)` pairs into a sorted
/// suggestion pool. Case-insensitive on the name (first-seen casing
/// wins); when duplicates collide, the higher count wins so a
/// freshly-imported `Sarah J. Maas` with 8 books beats a stale empty
/// `sarah j. maas` row. Output is alphabetic case-insensitive — feed
/// straight into [`ChipEditorProps::suggestions`].
pub fn collect_suggestions<I>(sources: I) -> Vec<SuggestionItem>
where
    I: IntoIterator<Item = SuggestionItem>,
{
    use std::collections::BTreeMap;
    let mut seen: BTreeMap<String, SuggestionItem> = BTreeMap::new();
    for item in sources {
        let key = item.name.to_lowercase();
        seen.entry(key)
            .and_modify(|existing| {
                if item.count > existing.count {
                    existing.count = item.count;
                }
            })
            .or_insert(item);
    }
    seen.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_suggestions_dedups_case_insensitively_and_keeps_higher_count() {
        let out = collect_suggestions([
            SuggestionItem::new("Sarah J. Maas", 8),
            SuggestionItem::new("sarah j. maas", 3),
            SuggestionItem::new("Brandon Sanderson", 12),
        ]);
        assert_eq!(out.len(), 2, "case-variant duplicates must collapse");
        let maas = out.iter().find(|i| i.name == "Sarah J. Maas").unwrap();
        assert_eq!(maas.count, 8, "higher count wins on collision");
        let sanderson = out.iter().find(|i| i.name == "Brandon Sanderson").unwrap();
        assert_eq!(sanderson.count, 12);
    }

    #[test]
    fn collect_suggestions_returns_sorted_output() {
        let out = collect_suggestions([
            SuggestionItem::new("Zelda", 0),
            SuggestionItem::new("Ada", 5),
            SuggestionItem::new("Mira", 1),
        ]);
        let names: Vec<&str> = out.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["Ada", "Mira", "Zelda"]);
    }
}
