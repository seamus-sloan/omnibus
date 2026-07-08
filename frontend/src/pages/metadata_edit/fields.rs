//! Page-local form-field primitives for the metadata edit form.
//!
//! Markup-only adapters consumed by `form_grid` and the page root: a
//! label slot with optional "EDITED" badge + hint, a single-line input,
//! and a multi-line textarea.

use dioxus::prelude::*;

/// Label with optional "EDITED" badge and hint text.
///
/// Renders `<label for=…>` when `target` is set (associates with an
/// input for screen readers), or a plain `<span>` when `target` is
/// empty (used by chip rows that don't focus a single control).
#[component]
pub(super) fn MeLabel(
    text: String,
    #[props(default)] edited: bool,
    #[props(default)] hint: String,
    /// The `id` of the input this label targets. Leave empty when the
    /// label heads a chip row or other multi-control group.
    #[props(default)]
    target: String,
) -> Element {
    if target.is_empty() {
        rsx! {
            span { class: "me-label",
                span { "{text}" }
                if edited {
                    span { class: "mono me-label-edited", "\u{b7} EDITED" }
                }
                if !hint.is_empty() {
                    span { class: "mono me-label-hint", "{hint}" }
                }
            }
        }
    } else {
        rsx! {
            label { class: "me-label", r#for: target,
                span { "{text}" }
                if edited {
                    span { class: "mono me-label-edited", "\u{b7} EDITED" }
                }
                if !hint.is_empty() {
                    span { class: "mono me-label-hint", "{hint}" }
                }
            }
        }
    }
}

/// Derive a stable input `id` from a label string (lowercase, hyphens for spaces).
pub(super) fn label_to_id(label: &str) -> String {
    format!(
        "me-{}",
        label
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
    )
}

/// Props for the [`MeField`] component.
#[derive(Props, Clone, PartialEq)]
pub(super) struct MeFieldProps {
    /// Field label; also slugified into the input's `id` and used as the
    /// fallback placeholder.
    pub label: String,
    /// The input's bound value signal.
    pub value: Signal<String>,
    /// Fired with the new text on every input.
    pub on_change: EventHandler<String>,
    /// Grid width in columns (`2` spans two columns; anything else is one).
    #[props(default)]
    pub w: i32,
    /// Big serif display styling (title field).
    #[props(default)]
    pub big: bool,
    /// Serif input styling.
    #[props(default)]
    pub serif: bool,
    /// Monospace input styling.
    #[props(default)]
    pub mono: bool,
    /// When true, applies the "edited" accent + badge.
    #[props(default)]
    pub edited: bool,
    /// Renders the input read-only + disabled.
    #[props(default)]
    pub locked: bool,
    /// Optional hint copy shown beside the label.
    #[props(default)]
    pub hint: String,
    /// Overrides the placeholder (defaults to the label when empty).
    #[props(default)]
    pub placeholder: String,
}

/// Single-line input field in the form grid.
#[component]
pub(super) fn MeField(props: MeFieldProps) -> Element {
    let MeFieldProps {
        label,
        value,
        on_change,
        w,
        big,
        serif,
        mono,
        edited,
        locked,
        hint,
        placeholder,
    } = props;
    let col_class = match w {
        2 => "me-field me-field-w2",
        _ => "me-field",
    };
    let input_class = if big && serif {
        "me-input me-input-big me-input-serif"
    } else if mono {
        "me-input me-input-mono"
    } else if serif {
        "me-input me-input-serif"
    } else {
        "me-input"
    };
    let border_class = if edited { " me-input-edited" } else { "" };

    let field_id = label_to_id(&label);

    rsx! {
        div { class: col_class,
            MeLabel { text: label.clone(), edited, hint, target: field_id.clone() }
            input {
                id: field_id,
                class: "{input_class}{border_class}",
                value: "{value}",
                placeholder: if placeholder.is_empty() { label } else { placeholder },
                readonly: locked,
                disabled: locked,
                oninput: move |e| on_change.call(e.value()),
            }
        }
    }
}

/// Props for the [`MeArea`] component.
#[derive(Props, Clone, PartialEq)]
pub(super) struct MeAreaProps {
    /// Field label; also slugified into the textarea's `id`.
    pub label: String,
    /// The textarea's bound value signal.
    pub value: Signal<String>,
    /// Fired with the new text on every input.
    pub on_change: EventHandler<String>,
    /// Visible row count for the textarea.
    #[props(default = 4)]
    pub rows: i32,
    /// When true, applies the "edited" accent + badge.
    #[props(default)]
    pub edited: bool,
    /// Optional hint copy shown beside the label.
    #[props(default)]
    pub hint: String,
}

/// Multi-line textarea field.
#[component]
pub(super) fn MeArea(props: MeAreaProps) -> Element {
    let MeAreaProps {
        label,
        value,
        on_change,
        rows,
        edited,
        hint,
    } = props;
    let border_class = if edited { " me-input-edited" } else { "" };
    let field_id = label_to_id(&label);

    rsx! {
        div { class: "me-field me-field-full",
            MeLabel { text: label.clone(), edited, hint, target: field_id.clone() }
            textarea {
                id: field_id,
                class: "me-textarea{border_class}",
                rows: "{rows}",
                value: "{value}",
                oninput: move |e| on_change.call(e.value()),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn label_to_id_slugifies_spaces_and_case() {
        assert_eq!(label_to_id("Title"), "me-title");
        assert_eq!(label_to_id("Book #"), "me-book--");
        assert_eq!(label_to_id("Published Date"), "me-published-date");
    }

    #[test]
    fn label_to_id_replaces_all_non_alphanumeric() {
        assert_eq!(label_to_id("A/B:C"), "me-a-b-c");
        assert_eq!(label_to_id(""), "me-");
    }
}
