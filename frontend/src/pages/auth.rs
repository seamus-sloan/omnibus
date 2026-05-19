//! Login and register pages for the web and mobile clients.
//!
//! Both pages render the same markup on every target so SSR/WASM hydration
//! matches. Submit handlers branch on feature:
//!
//! * `web` — POST JSON to `/api/auth/{login,register}` via `gloo-net`; the
//!   server sets the `omnibus_session` cookie via `Set-Cookie` and the
//!   browser's same-origin fetch keeps it for subsequent requests.
//! * `mobile` — POST JSON to the same endpoints via `reqwest`, with a
//!   `client_kind` of `ios` / `android` / `bearer` so the server issues a
//!   bearer token in the body. The token is stashed in
//!   `data::token_store` (only present under `feature = "mobile"`, hence
//!   this is a code-formatted reference rather than an intra-doc link)
//!   and attached to every subsequent request. Until secure storage
//!   lands, **debug builds only** persist the token in plaintext to
//!   `$HOME/.omnibus-token`; release builds keep it in memory and
//!   require re-login on each cold start. See the TODO at the top of
//!   that module.
//! * `server` — SSR never executes the submit closure (no interaction
//!   happens during SSR), so this path is a compile-only stub returning a
//!   static error.
//!
//! Polish ([F1.6](../../../../docs/roadmap/1-6-auth-ui.md)) wraps both
//! pages in [`AuthShell`] and replaces the bare `settings-field` divs with
//! the [`Field`] / [`Banner`] / [`StrengthMeter`] primitives from
//! [`crate::components::auth`]. The visual layer is presentational only —
//! server policy still owns password validation and session expiry.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};

use crate::components::auth::{AuthShell, Banner, BannerKind, Field, StrengthMeter, StrengthScore};
use crate::{use_server_url, Route};

#[component]
pub fn LoginPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut submitting = use_signal(|| false);
    let mut keep_signed_in = use_signal(|| false);
    let nav = use_navigator();

    // `use_server_url()` is feature-aware: empty string on web/server (where
    // requests are same-origin) and the `ServerUrl` context value on mobile.
    let server_url = use_server_url();

    let on_submit = move |evt: FormEvent| {
        evt.prevent_default();
        let u = username();
        let p = password();
        if u.is_empty() || p.is_empty() {
            error.set(Some("enter a username and password".into()));
            return;
        }
        error.set(None);
        submitting.set(true);
        let server_url = server_url.clone();
        spawn(async move {
            let res = submit_login(&server_url, u, p).await;
            submitting.set(false);
            match res {
                Ok(()) => {
                    nav.replace(Route::Landing {});
                }
                Err(e) => error.set(Some(e)),
            }
        });
    };

    rsx! {
        AuthShell {
            kicker: "Sign in".to_string(),
            title: rsx! {
                "Welcome "
                span { class: "auth-shell-headline-em", "back" }
            },
            lede: Some("Continue to your library.".to_string()),
            form { class: "auth-form-inner",
                onsubmit: on_submit,
                "data-testid": "login-form",
                if let Some(msg) = error() {
                    Banner {
                        kind: BannerKind::Err,
                        title: msg,
                        dismissible: false,
                    }
                }
                Field { label: "Username".to_string(),
                    input {
                        id: "login-username",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        value: "{username}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                Field {
                    label: "Password".to_string(),
                    // Stub: forgot-password page is P3-deferred (F5.4).
                    // Routing it back to /login keeps the affordance
                    // without dead routes.
                    action: rsx! {
                        Link {
                            to: Route::Login {},
                            class: "auth-field-action-link",
                            "Forgot?"
                        }
                    },
                    input {
                        id: "login-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                }
                label { class: "auth-checkbox",
                    input {
                        r#type: "checkbox",
                        checked: keep_signed_in(),
                        oninput: move |e| keep_signed_in.set(e.value() == "true"),
                    }
                    span { "Keep me signed in for 30 days" }
                }
                button {
                    class: "btn primary lg auth-submit",
                    r#type: "submit",
                    disabled: submitting(),
                    if submitting() { "Logging in…" } else { "Log in" }
                }
                p { class: "auth-footer",
                    "No account? "
                    Link { to: Route::Register {}, "Register" }
                }
                div { class: "auth-footer-note",
                    "omnibus · v"
                    {env!("CARGO_PKG_VERSION")}
                }
            }
        }
    }
}

#[component]
pub fn RegisterPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<RegisterError>::None);
    let mut submitting = use_signal(|| false);
    let mut terms_ack = use_signal(|| false);
    let nav = use_navigator();

    let server_url = use_server_url();

    let on_submit = move |evt: FormEvent| {
        evt.prevent_default();
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
    };

    let pw = password();
    let (score, score_label, rules) = score_password(&pw);
    let err = error();
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
    let has_error = err.is_some();
    let username_invalid = username_err.is_some();
    let password_invalid = password_err.is_some();
    let submit_label = if submitting() {
        "Creating…"
    } else if has_error {
        "Fix to continue"
    } else {
        "Create account"
    };

    rsx! {
        AuthShell {
            kicker: "Create account".to_string(),
            title: rsx! {
                "Make "
                span { class: "auth-shell-headline-em", "yourself" }
                " at home"
            },
            lede: Some("Set up your account to start using Omnibus.".to_string()),
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
                Field { label: "Username".to_string(), error: username_err,
                    input {
                        id: "register-username",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        value: "{username}",
                        aria_invalid: "{username_invalid}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                Field { label: "Password".to_string(), error: password_err,
                    input {
                        id: "register-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "new-password",
                        value: "{password}",
                        aria_invalid: "{password_invalid}",
                        oninput: move |e| password.set(e.value()),
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
                    disabled: submitting(),
                    "{submit_label}"
                }
                p { class: "auth-footer",
                    "Already have an account? "
                    Link { to: Route::Login {}, "Log in" }
                }
            }
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

// ---- presentational helpers ---------------------------------------------

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
    let label = match score.value() {
        0 => "empty",
        1 => "weak",
        2 => "fair",
        3 => "good",
        _ => "strong",
    };
    (score, label, [len_ok, mixed_case, has_number_or_symbol])
}

// ---- submit helpers ------------------------------------------------------
//
// Per-target HTTP transports for auth. The cfg gates are kept mutually
// exclusive within this file: the web impl compiles only for `web`
// builds without `mobile`, the mobile impl compiles for any `mobile`
// build, and the no-feature stub covers SSR-without-web. The
// `web` + `mobile` combination is rejected at crate level by a
// `compile_error!` in `frontend/src/components/mod.rs`, so this layer
// is defense-in-depth rather than the primary guard — but keeping the
// gates precise here means a future change that loosens the crate-level
// guard won't silently produce duplicate `submit_*` definitions.
// `server`-only builds (no `web` and no `mobile`) get a compile-only
// stub — SSR never executes the submit closure, so the stub is
// unreachable at runtime.

#[cfg(all(feature = "web", not(feature = "mobile")))]
async fn submit_login(_server_url: &str, username: String, password: String) -> Result<(), String> {
    use omnibus_shared::LoginRequest;
    crate::data::login(LoginRequest {
        username,
        password,
        client_kind: None,
        device_name: None,
        client_version: None,
    })
    .await
    .map(|_| ())
}

#[cfg(all(feature = "web", not(feature = "mobile")))]
async fn submit_register(
    _server_url: &str,
    username: String,
    password: String,
) -> Result<(), String> {
    use omnibus_shared::RegisterRequest;
    crate::data::register(RegisterRequest {
        username,
        password,
        client_kind: None,
        device_name: None,
        client_version: None,
    })
    .await
    .map(|_| ())
}

#[cfg(feature = "mobile")]
async fn submit_login(server_url: &str, username: String, password: String) -> Result<(), String> {
    crate::data::mobile_login(server_url, username, password, default_device_name())
        .await
        .map(|_| ())
}

#[cfg(feature = "mobile")]
async fn submit_register(
    server_url: &str,
    username: String,
    password: String,
) -> Result<(), String> {
    crate::data::mobile_register(server_url, username, password, default_device_name())
        .await
        .map(|_| ())
}

/// Best-effort device name for the bearer-login `device_name` field. The
/// value shows up in the admin UI's session list, so prefer something the
/// user will recognize. Until a settings screen lets the user override
/// this, we send a generic platform label.
#[cfg(feature = "mobile")]
fn default_device_name() -> Option<String> {
    let label = if cfg!(target_os = "ios") {
        "Omnibus iOS"
    } else if cfg!(target_os = "android") {
        "Omnibus Android"
    } else {
        "Omnibus Mobile"
    };
    Some(label.to_string())
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
async fn submit_login(
    _server_url: &str,
    _username: String,
    _password: String,
) -> Result<(), String> {
    Err("login is only available in the web or mobile client".into())
}

#[cfg(not(any(feature = "web", feature = "mobile")))]
async fn submit_register(
    _server_url: &str,
    _username: String,
    _password: String,
) -> Result<(), String> {
    Err("registration is only available in the web or mobile client".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_password_empty_is_zero() {
        let (score, label, rules) = score_password("");
        assert_eq!(score.value(), 0);
        assert_eq!(label, "empty");
        assert_eq!(rules, [false, false, false]);
    }

    #[test]
    fn score_password_grows_with_length_and_classes() {
        let (s, _, _) = score_password("abcd");
        assert_eq!(s.value(), 1);
        let (s, _, _) = score_password("AbCdEfGh");
        assert_eq!(s.value(), 3);
        let (s, label, rules) = score_password("AbCdEfGh1!2x");
        assert_eq!(s.value(), 4);
        assert_eq!(label, "strong");
        assert_eq!(rules, [true, true, true]);
    }

    #[test]
    fn score_password_rules_track_thresholds() {
        let (_, _, rules) = score_password("Ab1");
        assert_eq!(rules, [false, true, true]);
        let (_, _, rules) = score_password("abcdefghijk1");
        assert_eq!(rules, [true, false, true]);
    }

    #[test]
    fn classify_register_error_routes_username() {
        match classify_register_error("username already exists") {
            RegisterError::Username(m) => assert_eq!(m, "username already exists"),
            other => panic!("expected Username variant, got {other:?}"),
        }
    }

    #[test]
    fn classify_register_error_routes_password() {
        match classify_register_error("password is too short") {
            RegisterError::Password(m) => assert_eq!(m, "password is too short"),
            other => panic!("expected Password variant, got {other:?}"),
        }
    }

    #[test]
    fn classify_register_error_falls_back_to_other() {
        match classify_register_error("500: internal server error") {
            RegisterError::Other(m) => assert_eq!(m, "500: internal server error"),
            other => panic!("expected Other variant, got {other:?}"),
        }
    }
}
