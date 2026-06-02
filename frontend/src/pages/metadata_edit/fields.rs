//! Page-local form-field primitives for the metadata edit form.
//!
//! Markup-only adapters consumed by `form_grid` and the page root: a
//! label slot with optional "EDITED" badge + hint, a single-line input,
//! and a multi-line textarea.

use dioxus::prelude::*;

/// Label with optional "EDITED" badge and hint text.
/// Renders a `<label for=…>` so screen readers associate it with the input.
#[component]
pub(super) fn MeLabel(
    text: String,
    #[props(default)] edited: bool,
    #[props(default)] hint: String,
    /// The `id` of the input this label targets.
    #[props(default)]
    target: String,
) -> Element {
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

/// Single-line input field in the form grid.
#[component]
pub(super) fn MeField(
    label: String,
    value: Signal<String>,
    on_change: EventHandler<String>,
    #[props(default)] w: i32,
    #[props(default)] big: bool,
    #[props(default)] serif: bool,
    #[props(default)] mono: bool,
    #[props(default)] edited: bool,
    #[props(default)] locked: bool,
    #[props(default)] hint: String,
    #[props(default)] placeholder: String,
) -> Element {
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

/// Multi-line textarea field.
#[component]
pub(super) fn MeArea(
    label: String,
    value: Signal<String>,
    on_change: EventHandler<String>,
    #[props(default = 4)] rows: i32,
    #[props(default)] edited: bool,
    #[props(default)] hint: String,
) -> Element {
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
