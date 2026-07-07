//! Login page: form state/submission logic and the web/mobile shells.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};

#[cfg(not(feature = "mobile"))]
use crate::components::auth::AuthShell;
use crate::components::auth::{Banner, BannerKind, Field};
#[cfg(feature = "mobile")]
use crate::pages::server_connect::display_host;
use crate::{use_server_url, Route};

#[cfg(feature = "mobile")]
use super::m_auth_shell;
use super::submit_login;

/// Signals plus the submit/keydown handlers backing `LoginPage`'s form.
struct LoginFormState {
    username: Signal<String>,
    password: Signal<String>,
    error: Signal<Option<String>>,
    submitting: Signal<bool>,
    keep_signed_in: Signal<bool>,
    on_submit: EventHandler<FormEvent>,
    on_keydown: EventHandler<Event<KeyboardData>>,
}

/// Sets up the login form's signals and submission logic, decoupled from
/// the event source so both the form's `onsubmit` (click) and each
/// input's `onkeydown` (Enter) drive the same path — Dioxus 0.7's submit
/// event doesn't reliably fire on implicit form submission from Enter, so
/// we trigger it explicitly via a shared `use_callback` handle.
fn use_login_form_state() -> LoginFormState {
    let username = use_signal(String::new);
    let password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut submitting = use_signal(|| false);
    let keep_signed_in = use_signal(|| false);
    let nav = use_navigator();

    // `use_server_url()` is feature-aware: empty string on web/server (where
    // requests are same-origin) and the `ServerUrl` context value on mobile.
    let server_url = use_server_url();

    let submit_now = use_callback(move |_: ()| {
        if submitting() {
            return;
        }
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
    });

    LoginFormState {
        username,
        password,
        error,
        submitting,
        keep_signed_in,
        on_submit: EventHandler::new(move |evt: FormEvent| {
            evt.prevent_default();
            submit_now.call(());
        }),
        on_keydown: EventHandler::new(move |evt: Event<KeyboardData>| {
            if evt.key() == Key::Enter {
                evt.prevent_default();
                submit_now.call(());
            }
        }),
    }
}

/// Renders the login page.
#[component]
pub fn LoginPage() -> Element {
    let LoginFormState {
        username,
        password,
        error,
        submitting,
        keep_signed_in,
        on_submit,
        on_keydown,
    } = use_login_form_state();

    // The "keep me signed in" control only exists on the web split-pane; the
    // mobile design omits it. Read it on mobile so the signal isn't flagged
    // unused under `-D warnings`.
    #[cfg(feature = "mobile")]
    let _ = keep_signed_in;

    #[cfg(not(feature = "mobile"))]
    let out = rsx! {
        AuthShell {
            kicker: "Sign in".to_string(),
            title: rsx! {
                "Welcome "
                span { class: "auth-shell-headline-em", "back" }
            },
            lede: Some("Continue to your library.".to_string()),
            LoginForm {
                username,
                password,
                error,
                submitting,
                keep_signed_in,
                on_submit,
                on_keydown,
            }
        }
    };

    #[cfg(feature = "mobile")]
    let host = display_host(&use_server_url());
    #[cfg(feature = "mobile")]
    let out = m_auth_shell(
        "Your library, wherever you are.",
        rsx! {
            MobileLoginForm {
                host,
                username,
                password,
                error,
                submitting,
                on_submit,
                on_keydown,
            }
        },
    );

    out
}

/// Mobile login body: connected-server bar plus the credentials form. Split
/// out of `LoginPage` so the mobile branch reads as a plain composition.
#[cfg(feature = "mobile")]
#[component]
fn MobileLoginForm(
    host: String,
    mut username: Signal<String>,
    mut password: Signal<String>,
    error: Signal<Option<String>>,
    submitting: Signal<bool>,
    on_submit: EventHandler<FormEvent>,
    on_keydown: EventHandler<Event<KeyboardData>>,
) -> Element {
    let nav = use_navigator();
    rsx! {
        // Connected-to bar: shows which server this login targets, with a
        // Back affordance that returns to the Connect screen (which
        // pre-fills the current URL) without clearing it.
        div { class: "m-auth-connected",
            button {
                class: "m-auth-back",
                r#type: "button",
                onclick: move |_| { nav.replace(Route::ServerConnect {}); },
                "← Back"
            }
            div { class: "m-auth-connected-to",
                span { class: "m-auth-connected-dot" }
                span { class: "m-auth-connected-label",
                    "Logging into "
                    span { class: "mono", "{host}" }
                }
            }
        }
        form { class: "auth-form-inner",
            onsubmit: on_submit,
            "data-testid": "login-form",
            if let Some(msg) = error() {
                Banner { kind: BannerKind::Err, title: msg, dismissible: false }
            }
            Field { label: "Email or username".to_string(), input_id: "login-username".to_string(),
                input {
                    id: "login-username",
                    name: "username",
                    r#type: "text",
                    autocomplete: "username",
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    value: "{username}",
                    oninput: move |e| username.set(e.value()),
                    onkeydown: on_keydown,
                }
            }
            Field {
                label: "Password".to_string(),
                input_id: "login-password".to_string(),
                action: rsx! {
                    Link { to: Route::Login {}, class: "auth-field-action-link", "Forgot?" }
                },
                input {
                    id: "login-password",
                    name: "password",
                    r#type: "password",
                    autocomplete: "current-password",
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    value: "{password}",
                    oninput: move |e| password.set(e.value()),
                    onkeydown: on_keydown,
                }
            }
            button {
                class: "btn primary lg auth-submit",
                r#type: "submit",
                disabled: submitting(),
                if submitting() { "Signing in…" } else { "Sign in" }
            }
            p { class: "auth-footer",
                "New here? "
                Link { to: Route::Register {}, "Create an account" }
            }
        }
    }
}

/// Props for the [`LoginForm`] component.
#[derive(Props, Clone, PartialEq)]
struct LoginFormProps {
    /// Username input value, shared with the parent.
    username: Signal<String>,
    /// Password input value, shared with the parent.
    password: Signal<String>,
    /// Current submission error, if any.
    error: Signal<Option<String>>,
    /// True while a login request is in flight.
    submitting: Signal<bool>,
    /// "Keep me signed in" checkbox state.
    keep_signed_in: Signal<bool>,
    /// Fired on form submission (submit-button click or Enter).
    on_submit: EventHandler<FormEvent>,
    /// Fired on Enter keydown in either input.
    on_keydown: EventHandler<Event<KeyboardData>>,
}

/// Login form body — inputs write the parent's signals through,
/// submission delegates via `on_submit`/`on_keydown`.
#[component]
fn LoginForm(props: LoginFormProps) -> Element {
    let LoginFormProps {
        username,
        password,
        error,
        submitting,
        mut keep_signed_in,
        on_submit,
        on_keydown,
    } = props;
    rsx! {
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
            LoginCredentialFields { username, password, on_keydown }
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

/// Username + password inputs, split out of `LoginForm` so it reads as a
/// plain composition (the two fields always change together and share
/// `on_keydown`'s Enter-to-submit wiring).
#[component]
fn LoginCredentialFields(
    mut username: Signal<String>,
    mut password: Signal<String>,
    on_keydown: EventHandler<Event<KeyboardData>>,
) -> Element {
    rsx! {
        Field { label: "Username".to_string(), input_id: "login-username".to_string(),
            input {
                id: "login-username",
                name: "username",
                r#type: "text",
                autocomplete: "username",
                autocapitalize: "none",
                autocorrect: "off",
                spellcheck: "false",
                value: "{username}",
                oninput: move |e| username.set(e.value()),
                onkeydown: on_keydown,
            }
        }
        Field {
            label: "Password".to_string(),
            input_id: "login-password".to_string(),
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
                // iOS soft keyboards capitalize the first char, 401ing a case-sensitive password.
                autocapitalize: "none",
                autocorrect: "off",
                spellcheck: "false",
                value: "{password}",
                oninput: move |e| password.set(e.value()),
                onkeydown: on_keydown,
            }
        }
    }
}
