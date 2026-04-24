//! Login and register pages for the web client.
//!
//! Both pages render the same markup on SSR and in WASM so hydration matches.
//! The actual login/register action fires on form submit and only compiles
//! on the `web` feature. `server` / `mobile` builds still wire the submit
//! handler — it just returns a static error ("login is only available in
//! the web client") that the form renders via the usual error alert. The
//! `server` path is effectively unreachable because SSR never runs the
//! closure; the `mobile` stub is in place until PR4 ships the bearer flow.

use dioxus::prelude::*;
use dioxus_router::{use_navigator, Link};

use crate::Route;

#[component]
pub fn LoginPage() -> Element {
    let mut username = use_signal(String::new);
    let mut password = use_signal(String::new);
    let mut error = use_signal(|| Option::<String>::None);
    let mut submitting = use_signal(|| false);
    let nav = use_navigator();

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
        spawn(async move {
            let res = submit_login(u, p).await;
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
        spawn(async move {
            let res = submit_register(u, p).await;
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
// Only the `web` feature has a real HTTP transport for auth. `server` builds
// compile the component markup for SSR but never execute the submit closure
// (no interaction happens during SSR), so a compile-only stub is enough.
// Mobile has its own login flow (PR4), so the stub returns an error there.

#[cfg(feature = "web")]
async fn submit_login(username: String, password: String) -> Result<(), String> {
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

#[cfg(feature = "web")]
async fn submit_register(username: String, password: String) -> Result<(), String> {
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

#[cfg(not(feature = "web"))]
async fn submit_login(_username: String, _password: String) -> Result<(), String> {
    Err("login is only available in the web client".into())
}

#[cfg(not(feature = "web"))]
async fn submit_register(_username: String, _password: String) -> Result<(), String> {
    Err("registration is only available in the web client".into())
}
