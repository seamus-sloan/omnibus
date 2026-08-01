//! Login / register / logout / me transport. Mobile uses bearer tokens
//! via REST; web hits the same REST endpoints through `gloo-net` so
//! cookie handling stays on the existing CookieJar extractor. Login and
//! register diverge by name across the `#[cfg]` split (`mobile_login` vs
//! `login`); `logout` and `current_user` are shared names with per-target bodies.

#[cfg(any(feature = "web", feature = "mobile"))]
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};
// Server-only (no web) build still needs `UserSummary` for the SSR
// `current_user` stub below. The mobile-build `current_user` stub uses the
// import above; the wider `cfg` here keeps SSR self-sufficient without
// dragging in the unused `LoginRequest` / `LoginResponse` / `RegisterRequest`.
#[cfg(all(feature = "server", not(any(feature = "web", feature = "mobile"))))]
use omnibus_shared::UserSummary;

#[cfg(feature = "mobile")]
use super::{
    client_kind, drain_error, http_client, note_status, token_store, with_bearer, DataError,
};

// Mobile cannot use cookies (Dioxus Native is not a webview), so login
// requests carry `client_kind: "ios"|"android"|"bearer"`, which the server
// uses as the signal to issue a bearer token in the JSON response instead
// of a `Set-Cookie` header. The token is stashed in [`super::token_store`] and
// attached to every subsequent request via `with_bearer`.

/// POST `/api/auth/login` (mobile) — bearer-token login, stashes the token.
#[cfg(feature = "mobile")]
pub async fn mobile_login(
    server_url: &str,
    username: String,
    password: String,
    device_name: Option<String>,
) -> Result<UserSummary, DataError> {
    crate::data::require_online()?;
    let req = LoginRequest {
        username,
        password,
        client_kind: Some(client_kind().into()),
        device_name,
        client_version: Some(env!("CARGO_PKG_VERSION").into()),
    };
    finish_bearer_auth(post_mobile_auth(server_url, "/api/auth/login", &req).await?)
}

/// POST `/api/auth/register` (mobile) — bearer-token signup, stashes the token.
#[cfg(feature = "mobile")]
pub async fn mobile_register(
    server_url: &str,
    username: String,
    password: String,
    device_name: Option<String>,
) -> Result<UserSummary, DataError> {
    crate::data::require_online()?;
    let req = RegisterRequest {
        username,
        password,
        client_kind: Some(client_kind().into()),
        device_name,
        client_version: Some(env!("CARGO_PKG_VERSION").into()),
    };
    finish_bearer_auth(post_mobile_auth(server_url, "/api/auth/register", &req).await?)
}

/// Common tail for `mobile_login` / `mobile_register`: stash the bearer
/// token returned by the server and surface the user summary. Errors out
/// if the server didn't issue a token — that would indicate the
/// `client_kind` discriminator was missed server-side, which we want to
/// fail loudly rather than silently degrade to a no-auth state.
#[cfg(feature = "mobile")]
fn finish_bearer_auth(resp: LoginResponse) -> Result<UserSummary, DataError> {
    let Some(token) = resp.token else {
        return Err(DataError::Other(
            "server did not issue a bearer token".into(),
        ));
    };
    token_store::set(token);
    Ok(resp.user)
}

/// GET `/api/auth/me` (mobile) — resolve the bearer-authenticated user.
///
/// The mobile client discards the [`UserSummary`] returned at login, so the
/// Account screen re-fetches it here. A 401 clears the stored token (via
/// `note_status`) so `ScreenLayout` routes back to `/login`.
#[cfg(feature = "mobile")]
pub async fn get_me(server_url: &str) -> Result<UserSummary, DataError> {
    // Network-first on purpose: the account-switch wipe below keys on the
    // *fresh* identity. A cache-first answer right after a different user
    // logs in would return the previous user's "me", skip the wipe, and
    // leak their replicated data across accounts. Offline still serves the
    // cached identity instantly via the network-first helper's fast path.
    let me = crate::offline::cache::read_through_network_first(
        crate::offline::cache::keys::me(),
        get_me_online(server_url),
    )
    .await?;
    // Account switches wipe the previous user's replicated data (and
    // re-seed the "me" row this call just cached — see `note_user`).
    crate::offline::note_user(&me).await;
    Ok(me)
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_me_online(server_url: &str) -> Result<UserSummary, DataError> {
    let url = format!("{server_url}/api/auth/me");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<UserSummary>().await?)
}

/// GET `{server_url}/api/auth/registration` (mobile) — whether self-signup is
/// open, so the register screen can say so instead of presenting a form that
/// 403s. Unauthenticated and deliberately uncached: the answer gates a screen
/// the user is looking at right now, and an admin can flip it at any time.
#[cfg(feature = "mobile")]
pub async fn registration_status(server_url: &str) -> Result<bool, DataError> {
    let url = format!("{server_url}/api/auth/registration");
    let response = http_client().get(&url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response
        .json::<omnibus_shared::RegistrationStatus>()
        .await?
        .enabled)
}

/// POST `/api/auth/logout` (mobile) — best-effort revoke, then clear local token.
#[cfg(feature = "mobile")]
pub async fn mobile_logout(server_url: &str) -> Result<(), DataError> {
    // Best-effort server revocation, then always clear the local token so a
    // network failure can't leave the device wedged in a "logged in" state.
    let url = format!("{server_url}/api/auth/logout");
    let _ = with_bearer(http_client().post(&url)).send().await;
    token_store::clear();
    Ok(())
}

/// GET `{server_url}/api/_health` — a simple server-reachability probe used
/// by the pre-login Connect screen to confirm the entered URL points at a
/// live server before advancing to login. Any 2xx counts as reachable; the
/// body is ignored (the endpoint is a general unauthenticated liveness route,
/// not a mobile-specific one). No bearer — `/api/_health` is whitelisted in
/// the server's auth gate. A per-request timeout bounds the wait so a URL
/// that connects but never answers can't hang the screen forever.
///
/// Kept transport-generic on purpose (a plain reachability check other flows
/// could reuse); it currently compiles on the native/reqwest path where
/// `http_client` lives — a web caller would add a `gloo_net` branch here.
#[cfg(feature = "mobile")]
pub async fn check_server(server_url: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/_health");
    let response = http_client()
        .get(&url)
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await?;
    let status = response.status();
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// GET `{server_url}/api/_health` (mobile) — resolve the running server's
/// release version so the account "You" screen can show it alongside the
/// app's own compile-time version (#1055). Distinct from [`check_server`],
/// which only probes reachability and discards the body.
#[cfg(feature = "mobile")]
pub async fn get_server_version(server_url: &str) -> Result<String, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through("server_version".to_string(), async move {
        get_server_version_online(&url).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_server_version_online(server_url: &str) -> Result<String, DataError> {
    #[derive(serde::Deserialize)]
    struct HealthPayload {
        version: String,
    }
    let url = format!("{server_url}/api/_health");
    let response = http_client().get(&url).send().await?;
    let status = response.status();
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<HealthPayload>().await?.version)
}

#[cfg(feature = "mobile")]
async fn post_mobile_auth<T: serde::Serialize>(
    server_url: &str,
    path: &str,
    body: &T,
) -> Result<LoginResponse, DataError> {
    let url = format!("{server_url}{path}");
    let response = http_client().post(&url).json(body).send().await?;
    let status = response.status();
    if !status.is_success() {
        // Auth failures arrive here as the server's chosen status (400 bad
        // credentials, 409 duplicate username, …); 401 maps to the typed
        // `Unauthorized` variant for symmetry with `drain_error`. The body
        // is preserved in `Http` so the register-error classifier can still
        // route "username"/"password" diagnostics to the right field.
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<LoginResponse>().await?)
}

// The web client hits the REST auth endpoints directly via `gloo-net` rather
// than going through a Dioxus server function. The REST endpoints already
// know how to set/clear the `omnibus_session` cookie via the `CookieJar`
// extractor, and browser fetch (same-origin) round-trips the cookie
// automatically. Server functions would force us to re-plumb cookie
// handling through the Dioxus fullstack response shape for no gain.
//
// SSR and server-only builds don't need these helpers: the login/register
// pages render the same markup on the server (no auth calls issued during
// SSR), and the actions only fire on user interaction after hydration.

/// POST `/api/auth/login` (web) — cookie-session login; pings `web_auth_state` on success.
#[cfg(feature = "web")]
pub async fn login(req: LoginRequest) -> Result<LoginResponse, String> {
    let resp = post_auth_json("/api/auth/login", &req).await?;
    // Reset the auth-state channel so a prior 401 doesn't keep
    // ScreenLayout in redirect mode after a successful re-login.
    super::web_auth_state::notify_authorized();
    Ok(resp)
}

/// POST `/api/auth/register` (web) — cookie-session signup; pings `web_auth_state` on success.
#[cfg(feature = "web")]
pub async fn register(req: RegisterRequest) -> Result<LoginResponse, String> {
    let resp = post_auth_json("/api/auth/register", &req).await?;
    super::web_auth_state::notify_authorized();
    Ok(resp)
}

/// GET `/api/auth/registration` (web) — whether self-signup is open. Read by
/// the login page (to decide whether to offer a register link) and the
/// register page (to render a closed notice instead of a dead form), both of
/// which run before any session exists.
#[cfg(feature = "web")]
pub async fn registration_status() -> Result<bool, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/auth/registration")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("registration status failed: {}", res.status()));
    }
    res.json::<omnibus_shared::RegistrationStatus>()
        .await
        .map(|s| s.enabled)
        .map_err(|e| e.to_string())
}

/// POST `/api/settings/registration` (web) — admin open/close of self-signup.
/// The read counterpart is [`registration_status`]; only the write is
/// admin-gated server-side.
#[cfg(feature = "web")]
pub async fn set_registration_enabled(enabled: bool) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post("/api/settings/registration")
        .json(&omnibus_shared::RegistrationStatus { enabled })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return if body.trim().is_empty() {
            Err(format!("request failed ({status})"))
        } else {
            Err(body)
        };
    }
    Ok(())
}

/// POST `/api/auth/logout` (web) — clears the session cookie and notifies subscribers.
#[cfg(feature = "web")]
pub async fn logout() -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post("/api/auth/logout")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() && res.status() != 204 {
        return Err(format!("logout failed: {}", res.status()));
    }
    // A successful logout is the unauth path — same signal as a 401, so
    // ScreenLayout redirects to /login without each caller having to nav.
    super::web_auth_state::notify_unauthorized();
    Ok(())
}

/// Non-web stub for `logout`. Mirrors the `current_user` SSR/mobile
/// stub: the real REST call is web-only, but exposing a no-op under SSR
/// (and any non-web build that compiles `UserMenu`) lets the sign-out
/// closure call `data::logout()` unconditionally — so SSR and WASM emit
/// identical RSX (rule 07-hydration), without each call site needing a
/// `#[cfg]` gate inside its closure body.
#[cfg(all(any(feature = "server", feature = "mobile"), not(feature = "web")))]
pub async fn logout() -> Result<(), String> {
    Ok(())
}

/// GET `/api/auth/me` (web) — resolve the currently-authenticated user, if any.
#[cfg(feature = "web")]
pub async fn current_user() -> Result<Option<UserSummary>, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/auth/me")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status() == 401 {
        super::web_auth_state::notify_unauthorized();
        return Ok(None);
    }
    if !res.ok() {
        return Err(format!("me failed: {}", res.status()));
    }
    let user = res.json::<UserSummary>().await.map_err(|e| e.to_string())?;
    super::web_auth_state::notify_authorized();
    Ok(Some(user))
}

/// Non-web stub for `current_user`. The real `/api/auth/me` call is
/// web-only — under SSR (`feature = "server"` without `"web"`) there's no
/// browser cookie jar to resolve, and the mobile build never renders the
/// admin-only affordances driven off this signal. Returning `Ok(None)`
/// lets pages declare the `use_signal` + `use_effect` pair unconditionally
/// (so SSR and WASM hook counts match) without any admin surface leaking
/// into the prerendered markup or appearing on mobile.
#[cfg(all(any(feature = "server", feature = "mobile"), not(feature = "web")))]
pub async fn current_user() -> Result<Option<UserSummary>, String> {
    Ok(None)
}

/// SSR stubs for the registration read/write. Neither runs: the Users
/// settings section loads and mutates only after the WASM client mounts.
/// Present so its effect and click closure compile unconditionally, keeping
/// the hook count identical across targets (rule 07).
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn registration_status() -> Result<bool, String> {
    Ok(true)
}

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn set_registration_enabled(_enabled: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(feature = "web")]
async fn post_auth_json<T: serde::Serialize>(
    path: &str,
    body: &T,
) -> Result<LoginResponse, String> {
    use gloo_net::http::Request;
    let res = Request::post(path)
        .json(body)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        let status = res.status();
        let msg = res.text().await.unwrap_or_default();
        return Err(format!("{status}: {msg}"));
    }
    res.json::<LoginResponse>().await.map_err(|e| e.to_string())
}

#[cfg(all(test, feature = "mobile"))]
mod tests {
    // The state-lock guard is deliberately held across awaits: it serializes
    // whole async test bodies against process-global state, and each test owns
    // its own thread + runtime, so there is no interleaving to deadlock on.
    #![allow(clippy::await_holding_lock)]

    use super::*;

    #[tokio::test]
    async fn mobile_login_fails_fast_when_offline() {
        let _guard = crate::offline::sync::test_state_lock().lock().unwrap();
        crate::offline::sync::note_offline();
        let out = mobile_login(
            "http://127.0.0.1:1",
            "elena".into(),
            "correct-horse-battery".into(),
            None,
        )
        .await;
        assert!(
            matches!(out, Err(DataError::Offline)),
            "login must fast-fail without a network attempt"
        );
        crate::offline::sync::note_online();
    }
}
