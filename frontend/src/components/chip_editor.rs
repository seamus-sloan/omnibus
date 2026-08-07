//! Generic add/remove chip editor with a substring-match suggestion dropdown.
//! Renders chips, then input, then optional dropdown; the consumer wraps the
//! output in whatever flex container their layout needs. Up to
//! `MAX_SUGGESTIONS` (see [`super::suggestion_dropdown`]) case-insensitive
//! matches against `suggestions` show, excluding values already present.
//! Empty `suggestions` → free-text entry.

use dioxus::prelude::*;

use super::suggestion_dropdown::{compute_suggestions, DropdownSelectionState, SuggestionDropdown};
use crate::focus_after_paint::focus_after_paint;

/// Per-instance handle to the editor's own root DOM node, captured on mount
/// so a later blur can check whether focus is still somewhere inside this
/// subtree (see [`close_unless_focus_stayed_inside`]). `()` off the web
/// target: SSR never fires real DOM events, and mobile's WebView renderer
/// doesn't route through `dioxus::web`'s `WebEventExt`.
#[cfg(feature = "web")]
type ChipEditorRoot = web_sys::Element;
#[cfg(not(feature = "web"))]
type ChipEditorRoot = ();

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

/// Presentational options for [`ChipEditor`] — everything that shapes how the
/// chips, input, and dropdown look, split out of [`ChipEditorProps`] so the
/// component's props stay small (the values/suggestions/handlers stay on the
/// props; the knobs live here). Every field defaults, so a consumer can pass
/// `ChipEditorOptions::default()` and override only what differs.
#[derive(Clone, PartialEq)]
pub struct ChipEditorOptions {
    /// Placeholder shown inside the chip-input.
    pub placeholder: String,
    /// When true, each chip is prefixed with an uppercase-initials
    /// avatar. Used by author chips (`me-avatar`); off for tags.
    pub show_avatar: bool,
    /// CSS class for the inline `<input>`. Defaults to `me-chip-input`
    /// (the author/landing-row style); tag rows pass `me-tag-input`.
    pub input_class: String,
    /// Prefix for the per-chip Remove button's `aria-label`, e.g.
    /// "Remove" → "Remove Ada Lovelace" or "Remove tag" → "Remove tag
    /// fiction".
    pub aria_remove_prefix: String,
    /// Per-instance testid prefix so multiple ChipEditors on one page
    /// don't collide. The `<input>` gets `<prefix>-input` and the
    /// suggestions `<ul>` gets `<prefix>-suggestions`. Suggestion
    /// `<li>`s use `role="option"` with the suggestion as accessible
    /// name (no per-row testid) so Playwright resolves them via
    /// `getByRole("option", { name })`.
    pub testid_prefix: String,
    /// When `true`, the inline `<input>` receives `autofocus` so the
    /// dropdown surfaces on first paint. Used by the landing-page
    /// Authors cell so admins don't have to click twice (once to
    /// enter edit mode, once to focus the input). Off by default to
    /// keep the metadata-edit page from stealing focus from the page's
    /// other fields on mount.
    pub autofocus: bool,
    /// Optional uppercase mini-header rendered at the top of the
    /// suggestion dropdown — "ADD AUTHOR" / "ADD TAG". Empty string (the
    /// default) suppresses the header entirely.
    pub dropdown_header: String,
}

impl Default for ChipEditorOptions {
    fn default() -> Self {
        Self {
            placeholder: String::new(),
            show_avatar: false,
            input_class: "me-chip-input".to_string(),
            aria_remove_prefix: "Remove".to_string(),
            testid_prefix: "chip-editor".to_string(),
            autofocus: false,
            dropdown_header: String::new(),
        }
    }
}

/// Props for the [`ChipEditor`] component. The data + behavioral props live
/// here; presentational knobs are grouped in [`ChipEditorOptions`].
#[derive(Props, PartialEq, Clone)]
pub struct ChipEditorProps {
    /// The chip list. Shared with the consumer so the parent can read
    /// it for dirty-detection / persistence.
    pub values: Signal<Vec<String>>,
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
    /// Fired when the user presses Escape inside the input, or blurs away
    /// from the editor entirely (a genuine click-away, or Tab past the
    /// whole subtree — but not Tab onto a chip's own Remove button, nor a
    /// suggestion-row pick; see [`ChipEditor`]'s blur handling). Useful for
    /// host components that want to exit a wrapping edit mode in addition
    /// to clearing the dropdown highlight. Default no-op.
    #[props(default)]
    pub on_close: EventHandler<()>,
    /// Presentational knobs (placeholder, avatar, testids, …).
    #[props(default)]
    pub options: ChipEditorOptions,
}

/// Local reactive state for one [`ChipEditor`] instance. A custom-hook-style
/// helper — called unconditionally so Dioxus's per-scope hook-order tracking
/// sees the same `use_signal` sequence every render — so [`ChipEditor`]
/// itself starts from the derived selection instead of a wall of signal
/// declarations.
///
/// `focused` seeds from `autofocus`: the browser's autofocus does not always
/// emit `onfocus` for a freshly-mounted node (e.g. the landing cell's
/// editor, mounted on click), so without seeding the open-on-focus dropdown
/// never surfaces. `suppress_open` flips true after a commit so the
/// just-emptied input does not instantly re-surface the pool; any keystroke
/// / fresh focus clears it. `root_el` is captured by the wrapper's
/// `onmounted` in [`ChipEditor`]'s rsx; read by
/// `close_unless_focus_stayed_inside` on blur.
struct ChipEditorState {
    input: Signal<String>,
    highlight: Signal<Option<usize>>,
    focused: Signal<bool>,
    suppress_open: Signal<bool>,
    root_el: Signal<Option<ChipEditorRoot>>,
}

fn use_chip_editor_state(autofocus: bool) -> ChipEditorState {
    ChipEditorState {
        input: use_signal(String::new),
        highlight: use_signal(|| None),
        focused: use_signal(|| autofocus),
        suppress_open: use_signal(|| false),
        root_el: use_signal(|| None),
    }
}

/// Add/remove chip editor with autocomplete dropdown.
#[component]
pub fn ChipEditor(props: ChipEditorProps) -> Element {
    let ChipEditorState {
        mut input,
        mut highlight,
        mut focused,
        mut suppress_open,
        mut root_el,
    } = use_chip_editor_state(props.options.autofocus);

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

    rsx! {
        // `display: contents` (atrium.css) keeps this wrapper invisible to
        // the host's flex layout — it exists only so blur handling can tell
        // "focus left the whole editor" (chips + input + dropdown) apart
        // from "focus moved to another focusable element inside it" (e.g. a
        // chip's Remove button reached via Tab).
        div {
            class: "chip-editor-root",
            onmounted: move |evt: Event<MountedData>| capture_chip_editor_root(evt, &mut root_el),
            ChipList {
                values: values_sig,
                show_avatar: props.options.show_avatar,
                aria_remove_prefix: props.options.aria_remove_prefix.clone(),
                on_remove: move |new_values: Vec<String>| {
                    values_sig.set(new_values.clone());
                    on_change_remove.call(new_values);
                },
            }
            ChipInputArea {
                input,
                appearance: ChipInputAppearance {
                    placeholder: props.options.placeholder.clone(),
                    input_class: props.options.input_class.clone(),
                    testid_prefix: props.options.testid_prefix.clone(),
                    autofocus: props.options.autofocus,
                },
                dropdown: ChipDropdownView {
                    selection,
                    highlight: highlight(),
                    dropdown_header: props.options.dropdown_header.clone(),
                },
                callbacks: ChipInputAreaCallbacks {
                    input: ChipInputCallbacks {
                        on_focus: EventHandler::new(move |_| {
                            focused.set(true);
                            suppress_open.set(false);
                        }),
                        on_blur: EventHandler::new(move |evt: Event<FocusData>| {
                            focused.set(false);
                            highlight.set(None);
                            // Mirrors `EditableCell`'s onblur: a genuine click-away
                            // (or Tab past the whole editor) exits the host's
                            // wrapping edit mode the same way Escape does — but Tab
                            // *within* the editor (e.g. onto a chip's Remove
                            // button) must not, so this only closes when the
                            // element gaining focus falls outside `root_el`.
                            // Suggestion-row picks never reach here at all:
                            // `SuggestionDropdown`'s `onmousedown` calls
                            // `prevent_default()`, suppressing the browser's
                            // default focus-shift so the input never blurs during
                            // a pick.
                            close_unless_focus_stayed_inside(evt, root_el, on_close);
                        }),
                        on_input: EventHandler::new(move |value: String| {
                            input.set(value);
                            highlight.set(None);
                            suppress_open.set(false);
                        }),
                        on_keydown: EventHandler::new(on_keydown),
                    },
                    on_pick: EventHandler::new(move |name: String| commit(name)),
                },
            }
        }
    }
}

/// Store a handle to the editor's own root DOM node the first time it
/// mounts. No-op off the web target — see [`ChipEditorRoot`].
#[cfg(feature = "web")]
fn capture_chip_editor_root(evt: Event<MountedData>, root_el: &mut Signal<Option<ChipEditorRoot>>) {
    use dioxus::web::WebEventExt;
    if let Some(el) = evt.try_as_web_event() {
        root_el.set(Some(el));
    }
}

#[cfg(not(feature = "web"))]
fn capture_chip_editor_root(
    _evt: Event<MountedData>,
    _root_el: &mut Signal<Option<ChipEditorRoot>>,
) {
}

/// Close the host's wrapping edit mode unless the element gaining focus (the
/// blur event's `relatedTarget`) is still inside the editor's own root — see
/// the call site in [`ChipEditor`] for why that distinction matters. A
/// `relatedTarget` of `None` (focus landed on nothing focusable — the usual
/// case for clicking a plain heading or block of text) counts as "left the
/// editor" and closes it, matching real click-away behavior.
///
/// Off the web target, there is no `relatedTarget` to inspect, so this
/// always closes — reproducing the simple pre-fix behavior for mobile's
/// touch-first interaction model, where a physical Tab key isn't in play.
#[cfg(feature = "web")]
fn close_unless_focus_stayed_inside(
    evt: Event<FocusData>,
    root_el: Signal<Option<ChipEditorRoot>>,
    on_close: EventHandler<()>,
) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::JsCast;

    let stayed_inside = root_el
        .read()
        .as_ref()
        .zip(evt.try_as_web_event().and_then(|e| e.related_target()))
        .is_some_and(|(root, related)| {
            related
                .dyn_ref::<web_sys::Node>()
                .is_some_and(|node| root.contains(Some(node)))
        });
    if !stayed_inside {
        on_close.call(());
    }
}

#[cfg(not(feature = "web"))]
fn close_unless_focus_stayed_inside(
    _evt: Event<FocusData>,
    _root_el: Signal<Option<ChipEditorRoot>>,
    on_close: EventHandler<()>,
) {
    on_close.call(());
}

/// Presentational knobs shared by [`ChipInput`] and [`ChipInputArea`] —
/// `testid_prefix` is the raw prefix (not a final id) so each caller can
/// derive its own suffix (`-input`, `-suggestions`).
#[derive(Clone, PartialEq)]
struct ChipInputAppearance {
    placeholder: String,
    input_class: String,
    testid_prefix: String,
    autofocus: bool,
}

/// Suggestion-dropdown state + config for [`ChipInputArea`], decoupled from
/// the input's own appearance/callbacks.
#[derive(Clone, PartialEq)]
struct ChipDropdownView {
    selection: SelectionView,
    highlight: Option<usize>,
    dropdown_header: String,
}

/// [`ChipInputCallbacks`] plus the one extra callback [`ChipInputArea`] needs
/// for the dropdown's pick action.
#[derive(Clone, PartialEq)]
struct ChipInputAreaCallbacks {
    input: ChipInputCallbacks,
    on_pick: EventHandler<String>,
}

/// Props for the [`ChipInputArea`] sub-component. Presentational knobs,
/// dropdown state, and callbacks are each grouped into their own struct so
/// this stays under the 5-prop soft cap — mirroring how [`ChipEditorProps`]
/// groups its own presentational knobs into [`ChipEditorOptions`].
#[derive(Props, Clone, PartialEq)]
struct ChipInputAreaProps {
    input: Signal<String>,
    appearance: ChipInputAppearance,
    dropdown: ChipDropdownView,
    callbacks: ChipInputAreaCallbacks,
}

/// Input field plus its suggestion dropdown.
#[component]
fn ChipInputArea(props: ChipInputAreaProps) -> Element {
    let ChipInputAreaProps {
        input,
        appearance,
        dropdown,
        callbacks,
    } = props;
    let ChipDropdownView {
        selection,
        highlight,
        dropdown_header,
    } = dropdown;
    let SelectionView {
        filtered,
        typed,
        show_create_row,
    } = selection;
    let testid_prefix = appearance.testid_prefix.clone();
    rsx! {
        div { class: "chip-editor-input-wrap",
            ChipInput {
                input,
                appearance,
                callbacks: callbacks.input,
            }
            if !filtered.is_empty() || show_create_row {
                SuggestionDropdown {
                    selection: DropdownSelectionState {
                        filtered: filtered.clone(),
                        show_create_row,
                        typed: typed.clone(),
                        highlight,
                    },
                    dropdown_header,
                    testid: format!("{testid_prefix}-suggestions"),
                    on_pick: callbacks.on_pick,
                }
            }
        }
    }
}

/// Per-render derived view bundling the filtered suggestion rows, the typed text, and the Create-row flag.
#[derive(Clone, PartialEq)]
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

/// Focus/blur/input/keydown handlers shared by [`ChipInput`] and
/// [`ChipInputArea`] — every one forwards straight to the parent.
#[derive(Clone, PartialEq)]
struct ChipInputCallbacks {
    on_focus: EventHandler<()>,
    on_blur: EventHandler<Event<FocusData>>,
    on_input: EventHandler<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
}

/// Props for the [`ChipInput`] sub-component.
#[derive(Props, Clone, PartialEq)]
struct ChipInputProps {
    input: Signal<String>,
    appearance: ChipInputAppearance,
    callbacks: ChipInputCallbacks,
}

/// Inline `<input>` for the chip editor — owns nothing, forwards every event to the parent's handlers.
#[component]
fn ChipInput(props: ChipInputProps) -> Element {
    let ChipInputProps {
        input,
        appearance,
        callbacks,
    } = props;
    let ChipInputAppearance {
        placeholder,
        input_class,
        testid_prefix,
        autofocus,
    } = appearance;
    let testid = format!("{testid_prefix}-input");
    let ChipInputCallbacks {
        on_focus,
        on_blur,
        on_input,
        on_keydown,
    } = callbacks;
    rsx! {
        input {
            class: "{input_class}",
            "data-testid": "{testid}",
            placeholder: "{placeholder}",
            value: "{input}",
            autofocus,
            // The `autofocus` attribute alone does not reliably move real
            // DOM focus onto a node mounted post-hydration by a click
            // handler (as opposed to one present at initial page load) —
            // without this, blur-based close-on-click-away silently never
            // fires because the input was never actually focused to begin
            // with. Deferred to the next frame: calling `.focus()`
            // synchronously inside `onmounted` lands before layout
            // finishes and no-ops (same pattern as
            // `crate::focus_after_paint::focus_after_paint`).
            onmounted: move |evt: Event<MountedData>| {
                if autofocus {
                    focus_after_paint(&evt);
                }
            },
            onfocus: move |_| on_focus.call(()),
            onblur: move |evt: Event<FocusData>| on_blur.call(evt),
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
                    // Same reasoning as the suggestion rows' onmousedown:
                    // without prevent_default, clicking Remove blurs the
                    // input first, and (now that blur closes the editor)
                    // that unmounts this very button before its click event
                    // fires, so the chip never actually gets removed.
                    onmousedown: move |e: Event<MouseData>| e.prevent_default(),
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
