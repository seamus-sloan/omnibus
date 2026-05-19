use dioxus::prelude::*;

/// Form field with shared label / hint / error / success states.
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
///
/// Props:
/// - `label` — visible label text.
/// - `input_id` — the `id` set on the inner input element; drives the
///   label's `for=` binding so screen readers and Playwright's
///   `getByLabel` resolve the input by label text alone.
/// - `hint` — supporting copy under the input (hidden when an error is shown).
/// - `error` — when present, switches the field to its error visual and
///   shows the message under the input as `role="alert"`.
/// - `success` — when true, switches the field to its success visual
///   (green accent + check mark).
/// - `action` — optional right-aligned slot in the label row
///   (e.g. a "Forgot?" link).
/// - `children` — the input element.
#[component]
pub fn Field(
    label: String,
    input_id: String,
    #[props(default)] hint: Option<String>,
    #[props(default)] error: Option<String>,
    #[props(default = false)] success: bool,
    #[props(default)] action: Option<Element>,
    children: Element,
) -> Element {
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
