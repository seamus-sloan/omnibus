//! Auth pages: [`login::LoginPage`] and [`register::RegisterPage`] (both
//! re-exported below). This file keeps only what's shared between them: the
//! mobile [`m_auth_shell`] chrome and the per-target `submit_login`/
//! `submit_register` transports — web relies on the `Set-Cookie` session,
//! mobile stashes the returned bearer token in `data::token_store`.

use dioxus::prelude::*;

mod login;
mod register;

pub use login::LoginPage;
pub use register::RegisterPage;

/// Centered single-column shell for the mobile auth screens: brand mark,
/// tagline, a form slot, and a version footer over a soft accent glow.
#[cfg(feature = "mobile")]
pub(crate) fn m_auth_shell(tagline: &str, children: Element) -> Element {
    rsx! {
        div { class: "m-auth",
            div { class: "m-auth-brand",
                div { class: "auth-shell-brand-mark" }
                div { class: "auth-shell-brand-word", "Omnibus" }
                p { class: "m-auth-tagline", "{tagline}" }
            }
            div { class: "m-auth-body", {children} }
            div { class: "m-auth-foot mono",
                "omnibus · v"
                {env!("CARGO_PKG_VERSION")}
            }
        }
    }
}

// Per-target HTTP transports for auth. The cfg gates are kept mutually
// exclusive within this file: the web impl compiles only for `web` builds
// without `mobile`, the mobile impl compiles for any `mobile` build, and
// the no-feature stub covers SSR-without-web. The `web` + `mobile`
// combination is rejected at crate level by a `compile_error!` in
// `frontend/src/components/mod.rs`, so this layer is defense-in-depth
// rather than the primary guard — but keeping the gates precise here
// means a future change that loosens the crate-level guard won't silently
// produce duplicate `submit_*` definitions. `server`-only builds (no
// `web` and no `mobile`) get a compile-only stub — SSR never executes
// the submit closure, so the stub is unreachable at runtime.

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

/// Resolve a registration-open probe into the value the auth pages render.
///
/// A probe that *failed* reads as open. The server's `403` on
/// `POST /api/auth/register` is the real gate, so a status read that couldn't
/// complete — offline, a proxy hiccup, an SSR-only build — must never be what
/// hides the register form or its link. Erring the other way would lock
/// first-run users out of the only account-creation path the server has.
fn registration_open_or_default(probe: Result<bool, String>) -> bool {
    probe.unwrap_or(true)
}

/// Whether self-registration is open, for the login and register pages. Same
/// three-arm cfg split as `submit_register` above; SSR answers `None` so the
/// prerendered markup matches the first WASM paint (rule 07) and the real
/// value arrives from the post-mount effect.
#[cfg(all(feature = "web", not(feature = "mobile")))]
async fn fetch_registration_open(_server_url: &str) -> Result<bool, String> {
    crate::data::registration_status().await
}

#[cfg(feature = "mobile")]
async fn submit_login(server_url: &str, username: String, password: String) -> Result<(), String> {
    crate::data::mobile_login(server_url, username, password, default_device_name())
        .await
        .map(|_| ())
        .map_err(data_error_message)
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
        .map_err(data_error_message)
}

#[cfg(feature = "mobile")]
async fn fetch_registration_open(server_url: &str) -> Result<bool, String> {
    crate::data::registration_status(server_url)
        .await
        .map_err(data_error_message)
}

/// Flatten a [`crate::data::DataError`] into the user-facing string the auth
/// pages surface. Delegates to [`crate::data::server_error_message`], which
/// splices the server's diagnostic body back into an HTTP failure —
/// `DataError`'s own `Display` deliberately omits it, but the register-error
/// classifier keys on "username"/"password" substrings, so the mobile path
/// must keep the body to route field errors correctly (mirrors the pre-#96
/// `"{status}: {body}"` string).
#[cfg(feature = "mobile")]
fn data_error_message(err: crate::data::DataError) -> String {
    crate::data::server_error_message(&err)
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

#[cfg(not(any(feature = "web", feature = "mobile")))]
async fn fetch_registration_open(_server_url: &str) -> Result<bool, String> {
    Err("registration status is only available in the web or mobile client".into())
}

#[cfg(test)]
mod tests {
    use super::registration_open_or_default;

    #[test]
    fn registration_open_or_default_passes_through_a_successful_probe() {
        assert!(registration_open_or_default(Ok(true)));
        assert!(!registration_open_or_default(Ok(false)));
    }

    #[test]
    fn registration_open_or_default_treats_a_failed_probe_as_open() {
        // Fail-open is deliberate: the server still refuses the register POST,
        // and fail-closed would strand a first-run admin with no way in.
        assert!(registration_open_or_default(Err("network error".into())));
    }
}
