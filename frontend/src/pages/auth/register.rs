//! Register page: form validation, password scoring, and the web/mobile
//! shells.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};

#[cfg(not(feature = "mobile"))]
use crate::components::auth::AuthShell;
use crate::components::auth::{Banner, BannerKind, Field, StrengthMeter, StrengthScore};
use crate::{use_server_url, Route};

#[cfg(feature = "mobile")]
use super::m_auth_shell;
use super::submit_register;

/// Form-input signals shared across the register form body and its fields:
/// the two inputs, the routed error, the in-flight flag, and the terms
/// checkbox. Grouped so [`RegisterForm`] stays under the prop cap.
#[derive(Clone, Copy, PartialEq)]
struct RegisterFormState {
    username: Signal<String>,
    password: Signal<String>,
    error: Signal<Option<RegisterError>>,
    submitting: Signal<bool>,
    terms_ack: Signal<bool>,
}

/// Renders the register page.
#[component]
pub fn RegisterPage() -> Element {
    let username = use_signal(String::new);
    let password = use_signal(String::new);
    let mut error = use_signal(|| Option::<RegisterError>::None);
    let mut submitting = use_signal(|| false);
    let terms_ack = use_signal(|| false);
    let nav = use_navigator();

    let server_url = use_server_url();

    let submit_now = use_callback(move |_: ()| {
        if submitting() {
            return;
        }
        let u = username();
        let p = password();
        if u.is_empty() || p.is_empty() {
            error.set(Some(RegisterError::Other(
                "enter a username and password".into(),
            )));
            return;
        }
        error.set(None);
        submitting.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            let res = submit_register(&server_url, u, p).await;
            submitting.set(false);
            match res {
                Ok(()) => {
                    nav.replace(Route::Landing {});
                }
                Err(e) => error.set(Some(classify_register_error(&e))),
            }
        });
    });

    // Same target split as `LoginPage`: the register form body is shared
    // (`RegisterForm`), only the surrounding shell differs.
    let form = rsx! {
        RegisterForm {
            state: RegisterFormState {
                username,
                password,
                error,
                submitting,
                terms_ack,
            },
            on_submit_now: move |_| submit_now.call(()),
        }
    };

    #[cfg(not(feature = "mobile"))]
    let out = rsx! {
        AuthShell {
            kicker: "Create account".to_string(),
            title: rsx! {
                "Make "
                span { class: "auth-shell-headline-em", "yourself" }
                " at home"
            },
            lede: Some("Set up your account to start using Omnibus.".to_string()),
            {form}
        }
    };

    #[cfg(feature = "mobile")]
    let out = m_auth_shell("Make yourself at home.", form);

    out
}

/// Splits an `Option<RegisterError>` into the three field-specific
/// message slots the form renders: username, password, and the
/// top-of-form banner ("other").
fn classify_errors(
    err: &Option<RegisterError>,
) -> (Option<String>, Option<String>, Option<String>) {
    let username_err = err.as_ref().and_then(|e| match e {
        RegisterError::Username(m) => Some(m.clone()),
        _ => None,
    });
    let password_err = err.as_ref().and_then(|e| match e {
        RegisterError::Password(m) => Some(m.clone()),
        _ => None,
    });
    let other_err = err.as_ref().and_then(|e| match e {
        RegisterError::Other(m) => Some(m.clone()),
        _ => None,
    });
    (username_err, password_err, other_err)
}

/// Builds the register form's submit gate, shared by the form's
/// `onsubmit` (click) and each input's `onkeydown` (Enter): blocks
/// re-submission while a routed error is still showing — the same guard
/// as the submit button's `disabled` prop.
fn register_submit_handlers(
    submitting: Signal<bool>,
    error: Signal<Option<RegisterError>>,
    on_submit_now: EventHandler<()>,
) -> (EventHandler<FormEvent>, EventHandler<Event<KeyboardData>>) {
    let can_submit = move || !submitting() && error().is_none();
    let on_submit = EventHandler::new(move |evt: FormEvent| {
        evt.prevent_default();
        if can_submit() {
            on_submit_now.call(());
        }
    });
    let on_keydown = EventHandler::new(move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Enter {
            evt.prevent_default();
            if can_submit() {
                on_submit_now.call(());
            }
        }
    });
    (on_submit, on_keydown)
}

/// Register form body — inputs write the parent's signals through, submission delegates via `on_submit_now`.
#[component]
fn RegisterForm(state: RegisterFormState, on_submit_now: EventHandler<()>) -> Element {
    let RegisterFormState {
        username,
        password,
        error,
        submitting,
        mut terms_ack,
    } = state;
    let (on_submit, on_keydown) = register_submit_handlers(submitting, error, on_submit_now);

    let err = error();
    let (username_err, password_err, other_err) = classify_errors(&err);
    let has_error = err.is_some();
    let submit_label = if submitting() {
        "Creating…"
    } else if has_error {
        "Fix to continue"
    } else {
        "Create account"
    };

    rsx! {
        form { class: "auth-form-inner",
            onsubmit: on_submit,
            "data-testid": "register-form",
            if let Some(msg) = other_err {
                Banner {
                    kind: BannerKind::Err,
                    title: msg,
                    dismissible: false,
                }
            }
            UsernameField { username, error, username_err, on_keydown }
            PasswordSection { password, error, password_err, on_keydown }
            label { class: "auth-checkbox auth-checkbox-block",
                input {
                    r#type: "checkbox",
                    checked: terms_ack(),
                    oninput: move |e| terms_ack.set(e.value() == "true"),
                }
                span {
                    "I understand that the server admin can see my reading list, ratings, journals on shared shelves, and audiobook play position."
                }
            }
            button {
                class: "btn primary lg auth-submit",
                r#type: "submit",
                // Disable while submitting AND while a routed error is
                // shown — keeps users from immediately re-submitting
                // the same invalid form. Each input's `oninput` clears
                // the error signal so editing re-enables the button.
                disabled: submitting() || has_error,
                "{submit_label}"
            }
            p { class: "auth-footer",
                "Already have an account? "
                Link { to: Route::Login {}, "Log in" }
            }
        }
    }
}

/// Username field — the register form's only non-password input, split
/// out alongside `PasswordSection` so `RegisterForm` reads as a plain
/// composition of its fields.
#[component]
fn UsernameField(
    mut username: Signal<String>,
    mut error: Signal<Option<RegisterError>>,
    username_err: Option<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
) -> Element {
    let username_invalid = username_err.is_some();
    rsx! {
        Field {
            label: "Username".to_string(),
            input_id: "register-username".to_string(),
            error: username_err,
            input {
                id: "register-username",
                name: "username",
                r#type: "text",
                autocomplete: "username",
                autocapitalize: "none",
                autocorrect: "off",
                spellcheck: "false",
                value: "{username}",
                aria_invalid: "{username_invalid}",
                oninput: move |e| {
                    username.set(e.value());
                    error.set(None);
                },
                onkeydown: on_keydown,
            }
        }
    }
}

/// Password field + strength meter + the three-rule requirements
/// checklist. Split out of `RegisterForm` since the meter/checklist pair
/// always changes together with the password value.
#[component]
fn PasswordSection(
    mut password: Signal<String>,
    mut error: Signal<Option<RegisterError>>,
    password_err: Option<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
) -> Element {
    let password_invalid = password_err.is_some();
    let pw = password();
    let (score, score_label, rules) = score_password(&pw);

    rsx! {
        Field {
            label: "Password".to_string(),
            input_id: "register-password".to_string(),
            error: password_err,
            input {
                id: "register-password",
                name: "password",
                r#type: "password",
                autocomplete: "new-password",
                autocapitalize: "none",
                autocorrect: "off",
                spellcheck: "false",
                value: "{password}",
                aria_invalid: "{password_invalid}",
                onkeydown: on_keydown,
                oninput: move |e| {
                    password.set(e.value());
                    error.set(None);
                },
            }
        }
        StrengthMeter {
            score: score,
            label: Some(score_label.to_string()),
        }
        div { class: "auth-requirements",
            div { class: "auth-requirements-title", "Password needs" }
            PasswordRequirementRow { ok: rules[0], text: "At least 10 characters" }
            PasswordRequirementRow { ok: rules[1], text: "Mixed case" }
            PasswordRequirementRow { ok: rules[2], text: "One number or symbol" }
        }
    }
}

#[component]
fn PasswordRequirementRow(ok: bool, text: String) -> Element {
    let cls = if ok { "auth-req ok" } else { "auth-req" };
    let status = if ok { "met" } else { "not met" };
    rsx! {
        div { class: "{cls}",
            span { class: "auth-req-dot", aria_hidden: "true" }
            span { "{text}" }
            // Screen-reader-only status — the dot color alone isn't
            // perceivable to assistive tech, so announce met/unmet.
            span { class: "sr-only", ": {status}" }
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
enum RegisterError {
    Username(String),
    Password(String),
    Other(String),
}

/// Classify a flat error string into a field bucket. Heuristic — keys on
/// lowercase substring matches so trivial server-message wording changes
/// don't strand the UI. New variants ride the `Other` fallback (renders as
/// a top banner) rather than breaking field rendering.
fn classify_register_error(raw: &str) -> RegisterError {
    let lower = raw.to_lowercase();
    if lower.contains("username") || lower.contains("user already") {
        RegisterError::Username(raw.to_string())
    } else if lower.contains("password") {
        RegisterError::Password(raw.to_string())
    } else {
        RegisterError::Other(raw.to_string())
    }
}

/// Presentational password scoring — server still enforces policy (10-char
/// minimum). Returns the meter score (0..=4), a short label, and the three
/// checklist booleans (length≥10, mixed case, number-or-symbol) so the
/// page renders both the meter and the requirements list from one pass.
fn score_password(pw: &str) -> (StrengthScore, &'static str, [bool; 3]) {
    let len = pw.chars().count();
    let has_lower = pw.chars().any(|c| c.is_ascii_lowercase());
    let has_upper = pw.chars().any(|c| c.is_ascii_uppercase());
    let mixed_case = has_lower && has_upper;
    let has_number_or_symbol = pw
        .chars()
        .any(|c| c.is_ascii_digit() || !c.is_alphanumeric());
    let len_ok = len >= 10;

    let mut raw: u8 = 0;
    if len >= 4 {
        raw = raw.saturating_add(1);
    }
    if len >= 8 {
        raw = raw.saturating_add(1);
    }
    if mixed_case {
        raw = raw.saturating_add(1);
    }
    if has_number_or_symbol {
        raw = raw.saturating_add(1);
    }
    if len_ok {
        raw = raw.saturating_add(1);
    }
    let score = StrengthScore::new(raw.min(StrengthScore::MAX));
    // "empty" only when nothing was typed; a non-empty score-0 input
    // (1–3 chars, no special chars) is still weak, not empty.
    let label = if pw.is_empty() {
        "empty"
    } else {
        match score.value() {
            0 | 1 => "weak",
            2 => "fair",
            3 => "good",
            _ => "strong",
        }
    };
    (score, label, [len_ok, mixed_case, has_number_or_symbol])
}

#[cfg(test)]
mod tests;
