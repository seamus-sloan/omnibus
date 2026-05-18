use dioxus::prelude::*;

/// Form field with shared label / hint / error / success states.
///
/// Owns the `<label>` and the input wrapper so callers stop hand-rolling
/// label-input pairs with the same class triplet. Pass any input element
/// (`input`, `select`, `textarea`) as `children`.
///
/// Props:
/// - `label` — visible label text.
/// - `hint` — supporting copy under the input (hidden when an error is shown).
/// - `error` — when present, switches the field to its error visual and
///   shows the message under the input.
/// - `success` — when true, switches the field to its success visual
///   (green accent + check mark).
/// - `action` — optional right-aligned slot in the label row
///   (e.g. a "Forgot?" link).
/// - `children` — the input element.
#[component]
pub fn Field(
    label: String,
    #[props(default)] hint: Option<String>,
    #[props(default)] error: Option<String>,
    #[props(default = false)] success: bool,
    #[props(default)] action: Option<Element>,
    children: Element,
) -> Element {
    let state_class = field_state_class(error.as_deref(), success);
    let wrapper_class = format!("auth-field {state_class}");

    rsx! {
        label { class: "{wrapper_class}",
            div { class: "auth-field-label-row",
                span { class: "auth-field-label", "{label}" }
                if let Some(slot) = action {
                    span { class: "auth-field-action", {slot} }
                }
            }
            div { class: "auth-field-input-wrap",
                {children}
                if success {
                    span { class: "auth-field-check", "✓" }
                }
            }
            if let Some(msg) = error.as_deref() {
                div { class: "auth-field-msg auth-field-msg-err", role: "alert", "{msg}" }
            } else if let Some(text) = hint.as_deref() {
                div { class: "auth-field-msg auth-field-msg-hint", "{text}" }
            }
        }
    }
}

/// Resolve the modifier class for a field based on its error / success
/// props. Error wins over success when both are set; "neutral" is the
/// default when neither is set.
fn field_state_class(error: Option<&str>, success: bool) -> &'static str {
    match (error, success) {
        (Some(_), _) => "auth-field-err",
        (None, true) => "auth-field-ok",
        (None, false) => "auth-field-neutral",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_wins_over_success() {
        assert_eq!(field_state_class(Some("bad"), true), "auth-field-err");
    }

    #[test]
    fn success_when_no_error() {
        assert_eq!(field_state_class(None, true), "auth-field-ok");
    }

    #[test]
    fn neutral_when_neither() {
        assert_eq!(field_state_class(None, false), "auth-field-neutral");
    }

    #[test]
    fn empty_error_string_still_counts_as_error() {
        // An empty `Some("")` means the caller flagged the field as
        // invalid but had no copy ready — visual state stays "err" so
        // the field doesn't look fine just because the message is blank.
        assert_eq!(field_state_class(Some(""), false), "auth-field-err");
    }
}
