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

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};

use crate::{use_server_url, Route};

#[component]
pub fn LoginPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut submitting = use_signal(|| false);
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
        div { class: "auth-card card",
            h1 { "Log in" }
            p { class: "subtitle", "Welcome back to Omnibus." }
            form { class: "auth-form",
                onsubmit: on_submit,
                "data-testid": "login-form",
                div { class: "settings-field",
                    label { r#for: "login-username", "Username" }
                    input {
                        id: "login-username",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        value: "{username}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                div { class: "settings-field",
                    label { r#for: "login-password", "Password" }
                    input {
                        id: "login-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "current-password",
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                }
                if let Some(msg) = error() {
                    div { class: "error", role: "alert", "{msg}" }
                }
                button {
                    class: "btn",
                    r#type: "submit",
                    disabled: submitting(),
                    if submitting() { "Logging in…" } else { "Log in" }
                }
            }
            p { class: "auth-footer",
                "No account? "
                Link { to: Route::Register {}, "Register" }
            }
        }
    }
}

#[component]
pub fn RegisterPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut submitting = use_signal(|| false);
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
            let res = submit_register(&server_url, u, p).await;
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
        div { class: "auth-card card",
            h1 { "Create an account" }
            p { class: "subtitle", "Register to start using Omnibus." }
            form { class: "auth-form",
                onsubmit: on_submit,
                "data-testid": "register-form",
                div { class: "settings-field",
                    label { r#for: "register-username", "Username" }
                    input {
                        id: "register-username",
                        name: "username",
                        r#type: "text",
                        autocomplete: "username",
                        value: "{username}",
                        oninput: move |e| username.set(e.value()),
                    }
                }
                div { class: "settings-field",
                    label { r#for: "register-password", "Password" }
                    input {
                        id: "register-password",
                        name: "password",
                        r#type: "password",
                        autocomplete: "new-password",
                        value: "{password}",
                        oninput: move |e| password.set(e.value()),
                    }
                }
                if let Some(msg) = error() {
                    div { class: "error", role: "alert", "{msg}" }
                }
                button {
                    class: "btn",
                    r#type: "submit",
                    disabled: submitting(),
                    if submitting() { "Creating…" } else { "Create account" }
                }
            }
            p { class: "auth-footer",
                "Already have an account? "
                Link { to: Route::Login {}, "Log in" }
            }
        }
    }
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
