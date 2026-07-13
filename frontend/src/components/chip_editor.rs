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
    let mut focused = use_signal(|| props.options.autofocus);
    let mut suppress_open = use_signal(|| false);
    // Captured by the wrapper's `onmounted` below; read by
    // `close_unless_focus_stayed_inside` on blur.
    let mut root_el = use_signal(|| None::<ChipEditorRoot>);

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
                placeholder: props.options.placeholder.clone(),
                input_class: props.options.input_class.clone(),
                testid_prefix: props.options.testid_prefix.clone(),
                autofocus: props.options.autofocus,
                dropdown_header: props.options.dropdown_header.clone(),
                selection,
                highlight: highlight(),
                on_focus: move |_| {
                    focused.set(true);
                    suppress_open.set(false);
                },
                on_blur: move |evt: Event<FocusData>| {
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
                },
                on_input: move |value: String| {
                    input.set(value);
                    highlight.set(None);
                    suppress_open.set(false);
                },
                on_keydown,
                on_pick: move |name: String| commit(name),
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

/// Props for the [`ChipInputArea`] sub-component.
#[derive(Props, Clone, PartialEq)]
struct ChipInputAreaProps {
    input: Signal<String>,
    placeholder: String,
    input_class: String,
    testid_prefix: String,
    autofocus: bool,
    dropdown_header: String,
    selection: SelectionView,
    highlight: Option<usize>,
    on_focus: EventHandler<()>,
    on_blur: EventHandler<Event<FocusData>>,
    on_input: EventHandler<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
    on_pick: EventHandler<String>,
}

/// Input field plus its suggestion dropdown.
#[component]
fn ChipInputArea(props: ChipInputAreaProps) -> Element {
    let ChipInputAreaProps {
        input,
        placeholder,
        input_class,
        testid_prefix,
        autofocus,
        dropdown_header,
        selection,
        highlight,
        on_focus,
        on_blur,
        on_input,
        on_keydown,
        on_pick,
    } = props;
    let SelectionView {
        filtered,
        typed,
        show_create_row,
    } = selection;
    rsx! {
        div { class: "chip-editor-input-wrap",
            ChipInput {
                input,
                placeholder,
                input_class,
                testid: format!("{testid_prefix}-input"),
                autofocus,
                on_focus,
                on_blur,
                on_input,
                on_keydown,
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
                    on_pick,
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

/// Props for the [`ChipInput`] sub-component.
#[derive(Props, Clone, PartialEq)]
struct ChipInputProps {
    input: Signal<String>,
    placeholder: String,
    input_class: String,
    testid: String,
    autofocus: bool,
    on_focus: EventHandler<()>,
    on_blur: EventHandler<Event<FocusData>>,
    on_input: EventHandler<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
}

/// Inline `<input>` for the chip editor — owns nothing, forwards every event to the parent's handlers.
#[component]
fn ChipInput(props: ChipInputProps) -> Element {
    let ChipInputProps {
        input,
        placeholder,
        input_class,
        testid,
        autofocus,
        on_focus,
        on_blur,
        on_input,
        on_keydown,
    } = props;
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
            // `search_palette::focus_palette_input`).
            onmounted: move |evt: Event<MountedData>| {
                if autofocus {
                    focus_chip_input(evt);
                }
            },
            onfocus: move |_| on_focus.call(()),
            onblur: move |evt: Event<FocusData>| on_blur.call(evt),
            oninput: move |e| on_input.call(e.value()),
            onkeydown: move |e| on_keydown.call(e),
        }
    }
}

#[cfg(feature = "web")]
fn focus_chip_input(evt: Event<MountedData>) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// Non-web stub: SSR never paints an interactive input and mobile's touch
/// keyboard doesn't need the same rAF-deferred focus nudge. Defined so the
/// `onmounted` handler can call `focus_chip_input` unconditionally (rule
/// 07: hydration parity — keep cfg gates out of rsx bodies).
#[cfg(not(feature = "web"))]
fn focus_chip_input(_evt: Event<MountedData>) {}

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

/// Props for the [`SuggestField`] component.
#[derive(Props, Clone, PartialEq)]
pub struct SuggestFieldProps {
    /// The field's bound value — both the input's live text and the
    /// committed value. Unlike [`ChipEditorProps::values`] this is a single
    /// value, not a list: there is no separate "commit" step, typing
    /// directly edits the value and free text is always accepted.
    pub value: Signal<String>,
    /// Candidate pool; same shape as [`ChipEditorProps::suggestions`].
    pub suggestions: ReadSignal<Vec<SuggestionItem>>,
    /// The `<input>`'s `id`, so a sibling `<label for=...>` associates with it.
    pub id: String,
    /// CSS class for the `<input>`.
    #[props(default)]
    pub class: String,
    /// Placeholder shown when the value is empty.
    #[props(default)]
    pub placeholder: String,
    /// Per-instance testid prefix, same convention as
    /// [`ChipEditorOptions::testid_prefix`]: the input gets
    /// `<prefix>-input`, the dropdown `<prefix>-suggestions`.
    pub testid_prefix: String,
}

/// Single-value counterpart to [`ChipEditor`] for fields that hold one value
/// rather than a list (e.g. Series): the same substring-match suggestion
/// dropdown, but picking a row overwrites the field instead of adding a
/// chip, and there is no "+ Create" row since typing already edits the
/// value directly (free text is accepted as-is).
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
    let testid_prefix = props.testid_prefix.clone();

    rsx! {
        div { class: "chip-editor-input-wrap",
            input {
                id: props.id.clone(),
                class: "{props.class}",
                "data-testid": "{testid_prefix}-input",
                placeholder: "{props.placeholder}",
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
/// [`dispatch_keydown`] without the chip-list "+ Create" row: ↑/↓ move the
/// highlight, Enter overwrites `value` with the highlighted row and closes
/// the dropdown (or, with nothing highlighted, leaves the typed text — it's
/// already the live value), Escape drops the highlight and closes.
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
