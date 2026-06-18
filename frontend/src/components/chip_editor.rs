//! Generic add/remove chip editor with a substring-match suggestion dropdown.
//! Renders chips, then input, then optional dropdown; the consumer wraps the
//! output in whatever flex container their layout needs. Up to
//! [`MAX_SUGGESTIONS`] case-insensitive matches against `suggestions` show,
//! excluding values already present. Empty `suggestions` → free-text entry.

use dioxus::prelude::*;

/// Cap on suggestion rows. Five fits without scroll on a typical
/// edit-row layout and matches the "< 5 relevant suggestions"
/// user requirement.
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
    /// Build a [`SuggestionItem`] from a canonical name and its linked-book count (display-only).
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
    /// keep the metadata-edit page from stealing focus from the page's
    /// other fields on mount.
    #[props(default = false)]
    pub autofocus: bool,
    /// Optional uppercase mini-header rendered at the top of the
    /// suggestion dropdown — "ADD AUTHOR" / "ADD TAG". Empty string (the
    /// default) suppresses the header entirely.
    #[props(default)]
    pub dropdown_header: String,
    /// Fired when the user presses Escape inside the input. Useful for
    /// host components that want to exit a wrapping edit mode in
    /// addition to clearing the dropdown highlight. Default no-op.
    #[props(default)]
    pub on_close: EventHandler<()>,
}

/// Add/remove chip editor with autocomplete dropdown.
#[component]
pub fn ChipEditor(props: ChipEditorProps) -> Element {
    // Seed `focused` from `autofocus`: the browser's autofocus does not
    // always emit `onfocus` for a freshly-mounted node (e.g. the landing
    // cell's editor, mounted on click), so without seeding the
    // open-on-focus dropdown never surfaces. `suppress_open` flips true
    // after a commit so the just-emptied input does not instantly
    // re-surface the pool; any keystroke / fresh focus clears it.
    let mut input = use_signal(String::new);
    let mut highlight = use_signal::<Option<usize>>(|| None);
    let mut focused = use_signal(|| props.autofocus);
    let mut suppress_open = use_signal(|| false);

    let selection = compute_selection(&props, input(), focused(), suppress_open());
    let selection_kd = selection.clone();
    let mut values_sig = props.values;
    let on_change = props.on_change;
    let on_change_remove = on_change;
    let on_close = props.on_close;

    let mut commit = move |name: String| {
        commit_chip(
            name,
            &mut values_sig,
            &mut input,
            &mut highlight,
            &mut suppress_open,
            &on_change,
        );
    };
    let on_keydown = move |e: Event<KeyboardData>| {
        dispatch_keydown(
            e,
            &selection_kd,
            &mut input,
            &mut highlight,
            &mut commit,
            &on_close,
        );
    };
    let SelectionView {
        filtered,
        typed,
        show_create_row,
    } = selection;

    rsx! {
        Fragment {
            ChipList {
                values: values_sig,
                show_avatar: props.show_avatar,
                aria_remove_prefix: props.aria_remove_prefix.clone(),
                on_remove: move |new_values: Vec<String>| {
                    values_sig.set(new_values.clone());
                    on_change_remove.call(new_values);
                },
            }
            div { class: "chip-editor-input-wrap",
                ChipInput {
                    input,
                    placeholder: props.placeholder.clone(),
                    input_class: props.input_class.clone(),
                    testid: format!("{}-input", props.testid_prefix),
                    autofocus: props.autofocus,
                    on_focus: move |_| {
                        focused.set(true);
                        suppress_open.set(false);
                    },
                    on_blur: move |_| {
                        focused.set(false);
                        highlight.set(None);
                    },
                    on_input: move |value: String| {
                        input.set(value);
                        highlight.set(None);
                        suppress_open.set(false);
                    },
                    on_keydown,
                }
                if !filtered.is_empty() || show_create_row {
                    SuggestionDropdown {
                        selection: DropdownSelectionState {
                            filtered: filtered.clone(),
                            show_create_row,
                            typed: typed.clone(),
                            highlight: highlight(),
                        },
                        dropdown_header: props.dropdown_header.clone(),
                        testid: format!("{}-suggestions", props.testid_prefix),
                        on_pick: move |name: String| commit(name),
                    }
                }
            }
        }
    }
}

/// Per-render derived view bundling the filtered suggestion rows, the typed text, and the Create-row flag.
#[derive(Clone)]
struct SelectionView {
    filtered: Vec<SuggestionItem>,
    typed: String,
    show_create_row: bool,
}

/// Compute the visible suggestion rows + Create-row state for the current input/focus/values.
fn compute_selection(
    props: &ChipEditorProps,
    input_value: String,
    focused: bool,
    suppress_open: bool,
) -> SelectionView {
    // Read each signal once so `filtered` and `show_create_row` are derived
    // from a single consistent snapshot of `suggestions` and `values`.
    let suggestions = props.suggestions.read();
    let values = props.values.read();
    let query_lc = input_value.trim().to_lowercase();
    let filtered = compute_suggestions(&suggestions, &values, &query_lc, focused, suppress_open);
    let typed = input_value.trim().to_string();
    let show_create_row = should_show_create_row(&suggestions, &values, &query_lc, &typed);
    SelectionView {
        filtered,
        typed,
        show_create_row,
    }
}

/// Render the "+ Create '<query>'" footer row when the user has typed
/// something but no entry in the *full* suggestion pool (and no
/// currently-chosen value) is an exact case-insensitive match. Checking
/// against the truncated `filtered` view alone would let an exact-match
/// entry outside the top-5 truncation falsely surface a Create row for a
/// name that already exists in the library.
fn should_show_create_row(
    suggestions: &[SuggestionItem],
    current_values: &[String],
    query_lc: &str,
    typed: &str,
) -> bool {
    if typed.is_empty() || query_lc.is_empty() {
        return false;
    }
    let exact_match_in_pool = suggestions
        .iter()
        .any(|item| item.name.to_lowercase() == query_lc);
    let exact_match_in_values = current_values.iter().any(|v| v.to_lowercase() == query_lc);
    !exact_match_in_pool && !exact_match_in_values
}

/// Append `name` to `values` if non-empty and not already present
/// (case-insensitive). Resets `input`/`highlight` and suppresses
/// open-on-focus so the dropdown collapses against the now-empty input.
/// Calls `on_change` only on a successful add — duplicates are silently
/// dropped after clearing the input.
fn commit_chip(
    name: String,
    values: &mut Signal<Vec<String>>,
    input: &mut Signal<String>,
    highlight: &mut Signal<Option<usize>>,
    suppress_open: &mut Signal<bool>,
    on_change: &EventHandler<Vec<String>>,
) {
    let trimmed = name.trim().to_string();
    if trimmed.is_empty() {
        return;
    }
    // Dedup uses the same Unicode-aware `to_lowercase()` comparison the
    // filter loop runs against the suggestion pool, so a dropdown-surfaced
    // duplicate and a hand-typed duplicate are treated identically (the
    // old `eq_ignore_ascii_case` would let through case-variant duplicates
    // that contain non-ASCII).
    let trimmed_lc = trimmed.to_lowercase();
    if values.read().iter().any(|v| v.to_lowercase() == trimmed_lc) {
        input.set(String::new());
        highlight.set(None);
        suppress_open.set(true);
        return;
    }
    let mut new_values = values.read().clone();
    new_values.push(trimmed);
    values.set(new_values.clone());
    on_change.call(new_values);
    input.set(String::new());
    highlight.set(None);
    suppress_open.set(true);
}

/// Translate Enter / Arrow / Escape keystrokes into commits + highlight
/// moves. Total selectable rows are filtered suggestions plus an optional
/// "+ Create" trailing row at index `filtered.len()`; ↑/↓ wrap across the
/// whole set so the admin can land on Create with one keystroke after a
/// typed-from-scratch entry.
fn dispatch_keydown(
    e: Event<KeyboardData>,
    selection: &SelectionView,
    input: &mut Signal<String>,
    highlight: &mut Signal<Option<usize>>,
    commit: &mut impl FnMut(String),
    on_close: &EventHandler<()>,
) {
    let SelectionView {
        filtered,
        typed: create_value,
        show_create_row,
    } = selection;
    let show_create_row = *show_create_row;
    let create_row_index = filtered.len();
    let total_rows = filtered.len() + usize::from(show_create_row);
    match e.key() {
        Key::Enter => {
            e.prevent_default();
            // Stop Enter from bubbling to host handlers (e.g. the
            // landing table row's keydown that navigates to the
            // book detail page).
            e.stop_propagation();
            let typed_now = input();
            let value = match highlight() {
                Some(idx) if idx < filtered.len() => filtered
                    .get(idx)
                    .map(|s| s.name.clone())
                    .unwrap_or(typed_now),
                Some(idx) if idx == create_row_index && show_create_row => create_value.clone(),
                _ => typed_now,
            };
            commit(value);
        }
        Key::ArrowDown if total_rows > 0 => {
            e.prevent_default();
            e.stop_propagation();
            let next = match highlight() {
                Some(i) if i + 1 < total_rows => Some(i + 1),
                _ => Some(0),
            };
            highlight.set(next);
        }
        Key::ArrowUp if total_rows > 0 => {
            e.prevent_default();
            e.stop_propagation();
            let next = match highlight() {
                Some(0) | None => Some(total_rows - 1),
                Some(i) => Some(i - 1),
            };
            highlight.set(next);
        }
        Key::Escape => {
            e.stop_propagation();
            highlight.set(None);
            on_close.call(());
        }
        _ => {}
    }
}

/// Inline `<input>` for the chip editor — owns nothing, forwards every event to the parent's handlers.
#[component]
fn ChipInput(
    input: Signal<String>,
    placeholder: String,
    input_class: String,
    testid: String,
    autofocus: bool,
    on_focus: EventHandler<()>,
    on_blur: EventHandler<()>,
    on_input: EventHandler<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
) -> Element {
    rsx! {
        input {
            class: "{input_class}",
            "data-testid": "{testid}",
            placeholder: "{placeholder}",
            value: "{input}",
            autofocus,
            onfocus: move |_| on_focus.call(()),
            onblur: move |_| on_blur.call(()),
            oninput: move |e| on_input.call(e.value()),
            onkeydown: move |e| on_keydown.call(e),
        }
    }
}

/// Rendered chip row — one chip per value with an avatar (optional) and
/// remove button. Fires `on_remove` with the new full list after each removal.
#[component]
fn ChipList(
    values: Signal<Vec<String>>,
    show_avatar: bool,
    aria_remove_prefix: String,
    on_remove: EventHandler<Vec<String>>,
) -> Element {
    rsx! {
        for (i, value) in values.read().iter().cloned().enumerate() {
            div {
                class: "chip me-chip-item",
                key: "{i}-{value}",
                if show_avatar {
                    span { class: "me-avatar",
                        {value.chars().filter(|c| c.is_uppercase()).take(2).collect::<String>()}
                    }
                }
                // The label is its own element (not a bare text node) so
                // the chip exposes a node whose text is *exactly* the
                // value — `getByText(value, { exact: true })` in the E2E
                // specs would otherwise match nothing, since the chip
                // `div` also contains the avatar initials and the remove
                // button's "✕". Visually identical: `.chip` is an
                // inline-flex row, so the span is the same flex item the
                // bare text already was.
                span { class: "me-chip-label", "{value}" }
                button {
                    class: "me-chip-remove",
                    "aria-label": "{aria_remove_prefix} {value}",
                    onclick: move |_| {
                        let mut new_values = values.read().clone();
                        if i < new_values.len() {
                            new_values.remove(i);
                            on_remove.call(new_values);
                        }
                    },
                    "\u{2715}"
                }
            }
        }
    }
}

/// Per-render selection state for the dropdown: the visible suggestion
/// rows, whether a "+ Create" trailing row is shown, the raw text to
/// quote inside that row, and the keyboard highlight cursor. Grouped
/// so the dropdown signature stays compact as new pieces of selection
/// state accrete.
#[derive(Clone, PartialEq)]
struct DropdownSelectionState {
    filtered: Vec<SuggestionItem>,
    show_create_row: bool,
    typed: String,
    highlight: Option<usize>,
}

/// Autocomplete dropdown — filtered suggestion rows plus an optional
/// "+ Create" trailing row. `on_pick` fires with the chosen name.
#[component]
fn SuggestionDropdown(
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
fn compute_suggestions(
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

    #[test]
    fn compute_suggestions_returns_empty_when_pool_is_empty() {
        let result = compute_suggestions(&[], &[], "", true, false);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_suggestions_filters_already_chosen_values() {
        let pool = vec![
            SuggestionItem::new("Ada", 1),
            SuggestionItem::new("Mira", 2),
        ];
        let result = compute_suggestions(&pool, &["Ada".to_string()], "", true, false);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].name, "Mira");
    }

    #[test]
    fn compute_suggestions_returns_empty_when_unfocused_and_query_empty() {
        let pool = vec![SuggestionItem::new("Ada", 1)];
        let result = compute_suggestions(&pool, &[], "", false, false);
        assert!(result.is_empty());
    }

    #[test]
    fn compute_suggestions_returns_empty_when_suppress_open_is_true() {
        let pool = vec![SuggestionItem::new("Ada", 1)];
        let result = compute_suggestions(&pool, &[], "", true, true);
        assert!(result.is_empty());
    }
}
