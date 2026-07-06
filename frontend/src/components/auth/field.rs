//! Labeled form-field wrapper for the auth pages.
//!
//! Pairs a real `<label for=...>` with caller-supplied input markup and
//! optional hint / error / success / action slots. Used by the login and
//! register pages so every field has the same accessibility shape.

use dioxus::prelude::*;

/// Props for the [`Field`] component.
///
/// The outer element is a `<div>`, and the visible label is a real
/// `<label for={input_id}>`. **The error / hint message is rendered as a
/// sibling of the input wrapper, not inside the label** — wrapping the
/// alert in the `<label>` (as a prior version did) pollutes the input's
/// accessible name, so `getByLabel("Password", { exact: true })` stops
/// matching as soon as an error appears.
///
/// Callers pass any input element (`input`, `select`, `textarea`) as
/// `children` and the matching `input_id` so the `<label>` can bind via
/// `for=`.
#[derive(Props, Clone, PartialEq)]
pub struct FieldProps {
    /// Visible label text.
    pub label: String,
    /// The `id` set on the inner input element; drives the label's
    /// `for=` binding so screen readers and Playwright's `getByLabel`
    /// resolve the input by label text alone.
    pub input_id: String,
    /// Supporting copy under the input (hidden when an error is shown).
    #[props(default)]
    pub hint: Option<String>,
    /// When present, switches the field to its error visual and shows
    /// the message under the input as `role="alert"`.
    #[props(default)]
    pub error: Option<String>,
    /// When true, switches the field to its success visual (green accent
    /// + check mark).
    #[props(default = false)]
    pub success: bool,
    /// Optional slot rendered visually at the top-right of the field
    /// (e.g. a "Forgot?" link). Lives **after** the input in DOM order
    /// and is positioned by CSS, so tabbing through the form goes
    /// label → input → action → next field rather than label → action →
    /// input (the latter pulls focus to the action between fields and
    /// traps typed characters the user meant for the input).
    #[props(default)]
    pub action: Option<Element>,
    /// The input element.
    pub children: Element,
}

/// Form field with shared label / hint / error / success states. See
/// [`FieldProps`] for the field contract.
#[component]
pub fn Field(props: FieldProps) -> Element {
    let FieldProps {
        label,
        input_id,
        hint,
        error,
        success,
        action,
        children,
    } = props;
    let state_class = field_state_class(error.as_deref(), success);
    let wrapper_class = format!("auth-field {state_class}");

    // A stable `data-testid` on the wrapper lets Playwright scope alerts /
    // hints to a specific field without resorting to XPath or class-based
    // ancestor walks (which violate `.claude/rules/04-playwright.md`).
    let field_testid = format!("{input_id}-field");

    rsx! {
        div { class: "{wrapper_class}", "data-testid": "{field_testid}",
            div { class: "auth-field-label-row",
                label { class: "auth-field-label", r#for: "{input_id}", "{label}" }
            }
            div { class: "auth-field-input-wrap",
                {children}
                if success {
                    span { class: "auth-field-check", "✓" }
                }
            }
            if let Some(slot) = action {
                span { class: "auth-field-action", {slot} }
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
