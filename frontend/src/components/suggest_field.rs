//! Single-value counterpart to [`super::chip_editor::ChipEditor`] for fields
//! that hold one value rather than a list (e.g. Series): the same
//! substring-match suggestion dropdown, but picking a row overwrites the
//! field instead of adding a chip, and there is no "+ Create" row since
//! typing already edits the value directly (free text is accepted as-is).

use dioxus::prelude::*;

use super::chip_editor::SuggestionItem;
use super::suggestion_dropdown::{compute_suggestions, DropdownSelectionState, SuggestionDropdown};

/// Presentational knobs for [`SuggestField`] — CSS class, placeholder, and
/// testid prefix — split out of [`SuggestFieldProps`] so the component's
/// props stay at the established cap, mirroring how
/// [`super::chip_editor::ChipEditorOptions`] splits the same kind of knobs
/// out of `ChipEditorProps` for its sibling component.
#[derive(Clone, PartialEq, Default)]
pub struct SuggestFieldOptions {
    /// CSS class for the `<input>`.
    pub class: String,
    /// Placeholder shown when the value is empty.
    pub placeholder: String,
    /// Per-instance testid prefix, same convention as
    /// [`super::chip_editor::ChipEditorOptions::testid_prefix`]: the input
    /// gets `<prefix>-input`, the dropdown `<prefix>-suggestions`.
    pub testid_prefix: String,
}

/// Props for the [`SuggestField`] component.
#[derive(Props, Clone, PartialEq)]
pub struct SuggestFieldProps {
    /// The field's bound value — both the input's live text and the
    /// committed value. Unlike [`super::chip_editor::ChipEditorProps::values`]
    /// this is a single value, not a list: there is no separate "commit"
    /// step, typing directly edits the value and free text is always
    /// accepted.
    pub value: Signal<String>,
    /// Candidate pool; same shape as
    /// [`super::chip_editor::ChipEditorProps::suggestions`].
    pub suggestions: ReadSignal<Vec<SuggestionItem>>,
    /// The `<input>`'s `id`, so a sibling `<label for=...>` associates with it.
    pub id: String,
    /// Presentational knobs (class, placeholder, testid prefix).
    #[props(default)]
    pub options: SuggestFieldOptions,
}

/// Single-value counterpart to [`super::chip_editor::ChipEditor`] — picking a
/// suggestion overwrites the field instead of adding a chip.
#[component]
pub fn SuggestField(props: SuggestFieldProps) -> Element {
    let mut value = props.value;
    let mut highlight = use_signal::<Option<usize>>(|| None);
    // Explicit open-state, mirroring how `ChipEditor` uses `focused` +
    // `suppress_open` to gate its dropdown. `filtered` being non-empty is
    // *not* sufficient here: after a pick the value is set to the chosen
    // name, which is often a substring of other pool entries, so the filter
    // would keep matching and the dropdown would never close. `open` is set
    // false on pick / Enter / Escape and back true on focus / input.
    let mut open = use_signal(|| false);

    let filtered = {
        let suggestions = props.suggestions.read();
        let query_lc = value().trim().to_lowercase();
        compute_suggestions(&suggestions, &[value()], &query_lc, open(), false)
    };
    let total = filtered.len();
    let testid_prefix = props.options.testid_prefix.clone();

    rsx! {
        div { class: "chip-editor-input-wrap",
            input {
                id: props.id.clone(),
                class: "{props.options.class}",
                "data-testid": "{testid_prefix}-input",
                placeholder: "{props.options.placeholder}",
                value: "{value}",
                onfocus: move |_| open.set(true),
                onblur: move |_| {
                    open.set(false);
                    highlight.set(None);
                },
                oninput: move |e| {
                    value.set(e.value());
                    highlight.set(None);
                    open.set(true);
                },
                onkeydown: move |e: Event<KeyboardData>| {
                    dispatch_suggest_field_keydown(e, &filtered, &mut value, &mut highlight, &mut open, total);
                },
            }
            if open() && !filtered.is_empty() {
                SuggestionDropdown {
                    selection: DropdownSelectionState {
                        filtered: filtered.clone(),
                        show_create_row: false,
                        typed: value(),
                        highlight: highlight(),
                    },
                    dropdown_header: String::new(),
                    testid: format!("{testid_prefix}-suggestions"),
                    on_pick: move |name: String| {
                        value.set(name);
                        highlight.set(None);
                        // Close on pick: without this the just-filled value
                        // (a substring of other pool entries) keeps the
                        // filter non-empty and the dropdown open.
                        open.set(false);
                    },
                }
            }
        }
    }
}

/// Arrow/Enter/Escape handling for [`SuggestField`] — a smaller sibling of
/// `chip_editor`'s `dispatch_keydown` without the chip-list "+ Create" row:
/// ↑/↓ move the highlight, Enter overwrites `value` with the highlighted row
/// and closes the dropdown (or, with nothing highlighted, leaves the typed
/// text — it's already the live value), Escape drops the highlight and
/// closes.
fn dispatch_suggest_field_keydown(
    e: Event<KeyboardData>,
    filtered: &[SuggestionItem],
    value: &mut Signal<String>,
    highlight: &mut Signal<Option<usize>>,
    open: &mut Signal<bool>,
    total: usize,
) {
    match e.key() {
        Key::ArrowDown if total > 0 => {
            e.prevent_default();
            let next = match highlight() {
                Some(i) if i + 1 < total => Some(i + 1),
                _ => Some(0),
            };
            highlight.set(next);
        }
        Key::ArrowUp if total > 0 => {
            e.prevent_default();
            let next = match highlight() {
                Some(0) | None => Some(total - 1),
                Some(i) => Some(i - 1),
            };
            highlight.set(next);
        }
        Key::Enter => {
            if let Some(item) = highlight().and_then(|idx| filtered.get(idx)) {
                e.prevent_default();
                value.set(item.name.clone());
                highlight.set(None);
                open.set(false);
            }
        }
        Key::Escape => {
            highlight.set(None);
            open.set(false);
        }
        _ => {}
    }
}
