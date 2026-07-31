//! HTTP plumbing under the data wrappers: the shared mobile `reqwest`
//! clients with their bearer/401 helpers, and the web-side auth-state
//! channel the server-function error path pings.

#[cfg(feature = "mobile")]
use super::stores::token_store;
use super::DataError;

/// Best-effort `client_kind` for the bearer-login request body, used
/// server-side to label the device and decide cookie vs. bearer issuance.
#[cfg(feature = "mobile")]
pub(crate) fn client_kind() -> &'static str {
    if cfg!(target_os = "ios") {
        "ios"
    } else if cfg!(target_os = "android") {
        "android"
    } else {
        "bearer"
    }
}

/// Shared, lazily-initialized HTTP client. Used for both authenticated
/// data calls (which thread the bearer through `with_bearer`) and the
/// pre-auth login/register/logout calls in `post_mobile_auth`. Reusing
/// one client keeps connection pooling, TLS sessions, and keep-alives
/// hot — important on mobile where each cold-start handshake hits
/// battery and latency hard. `Client` is internally `Arc`'d, so
/// `.clone()` is cheap.
#[cfg(feature = "mobile")]
pub(crate) fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            // 5s connect keeps dead-network requests from waiting out the OS
            // connect timeout (~75s on iOS); 30s total matches the server's
            // own request timeout.
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|e| {
                    // A degraded client (no timeouts → long offline stalls)
                    // must at least be diagnosable.
                    tracing::warn!(error = %e, "http client builder failed; using default client without timeouts");
                    reqwest::Client::new()
                })
        })
        .clone()
}

/// Client for long-lived streaming transfers (the download engine). No
/// whole-request timeout — a multi-GB audiobook legitimately outlives any
/// sane cap; a stalled stream is caught by the read timeout instead.
#[cfg(feature = "mobile")]
pub(crate) fn streaming_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .read_timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|e| {
                    tracing::warn!(error = %e, "streaming client builder failed; using default client without timeouts");
                    reqwest::Client::new()
                })
        })
        .clone()
}

/// Fast-fail guard for online-only mobile operations (login, download
/// start, uploads, send-to-Kindle): errors instantly with
/// [`DataError::Offline`] while the client is known-offline, instead of
/// burning a doomed connect attempt. Never used on queued (`write_through`)
/// or cached (`read_through`) paths — those have their own offline handling.
#[cfg(feature = "mobile")]
pub(crate) fn require_online() -> Result<(), DataError> {
    if crate::offline::sync::is_offline() {
        return Err(DataError::Offline);
    }
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) fn with_bearer(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    if let Some(token) = token_store::get() {
        rb.bearer_auth(token)
    } else {
        rb
    }
}

/// Inspect a response: if it's a 401, clear the stored bearer token so the
/// next render of the auth-aware UI can route to `/login`. Returns the same
/// status the caller was about to inspect.
///
/// The server is the sole authority on session expiry (bearer TTL is 90 days);
/// this 401 path is the *only* logout trigger. Do not add a client-side clock
/// — the persisted token intentionally survives cold starts.
#[cfg(feature = "mobile")]
pub(crate) fn note_status(status: reqwest::StatusCode) -> reqwest::StatusCode {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        token_store::clear();
    }
    status
}

/// Map a non-success response into a typed [`DataError`]. A 401 becomes
/// [`DataError::Unauthorized`] (so callers can pattern-match the auth path);
/// everything else drains the body into [`DataError::Http`]. Always reading
/// the body — even on the error path — lets reqwest return the underlying TCP
/// connection to its pool instead of dropping it mid-stream, and folds the
/// server's diagnostic text into the structured error.
///
/// Precondition: only call on a non-success status. The authenticated data
/// calls run `note_status` first, so the bearer token is already cleared by
/// the time we land here on a 401. The pre-auth `post_mobile_auth` path does
/// not call `note_status`, but a pre-auth 401 has no stored token to clear.
#[cfg(feature = "mobile")]
pub(crate) async fn drain_error(
    response: reqwest::Response,
    status: reqwest::StatusCode,
) -> DataError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return DataError::Unauthorized;
    }
    let body = response.text().await.unwrap_or_default();
    DataError::Http {
        status: status.as_u16(),
        body,
    }
}

#[cfg(feature = "web")]
pub mod web_auth_state {
    //! Reactive web-side auth-state channel used by `ScreenLayout` to
    //! redirect to `/login` whenever a data call surfaces a 401.
    //!
    //! The web counterpart to [`super::token_store::subscribe`]: the web
    //! client uses session cookies (round-tripped automatically by the
    //! browser) so there's no client-side token to clear, but the router
    //! still needs a reactive signal to redirect to `/login` when any
    //! data-layer call returns 401 (session expired, server restarted,
    //! admin revoked). All web data wrappers route their errors through
    //! [`super::note_server_fn_err`], which pushes `false` onto this
    //! channel on a 401 response; `ScreenLayout` subscribes and
    //! `nav.replace`s.

    use std::sync::OnceLock;
    use tokio::sync::watch;

    fn channel() -> &'static (watch::Sender<bool>, watch::Receiver<bool>) {
        static CH: OnceLock<(watch::Sender<bool>, watch::Receiver<bool>)> = OnceLock::new();
        CH.get_or_init(|| watch::channel(true))
    }

    /// Returns a receiver that observes auth state. `true` = currently
    /// believed-authenticated, `false` = a recent request returned 401.
    pub fn subscribe() -> watch::Receiver<bool> {
        channel().0.subscribe()
    }

    /// Signal that the most recent data call returned 401. `send_replace`
    /// doesn't require active receivers and never errors, so this is safe
    /// to call from any async context.
    pub fn notify_unauthorized() {
        channel().0.send_replace(false);
    }

    /// Signal that we've just observed an authenticated state — a fresh
    /// login/register succeeded, or `/api/auth/me` confirmed an existing
    /// session. Without this, the channel would latch at `false` after
    /// the first 401 and stay there for the WASM instance's lifetime, so
    /// a re-login from the redirected-to /login page couldn't reactively
    /// re-mount protected screens.
    pub fn notify_authorized() {
        channel().0.send_replace(true);
    }
}

/// Inspect a server-function error (Dioxus wraps it in `CapturedError`,
/// which holds an `Arc<anyhow::Error>`) and — on the web client — ping
/// `web_auth_state` if the underlying `ServerFnError` carries a 401
/// status code. Maps the error into a [`DataError`]: a 401 becomes
/// [`DataError::Unauthorized`] (so the web side can pattern-match the auth
/// path the same way mobile does), and any other failure is preserved as a
/// stringified [`DataError::Other`]. SSR builds (cfg(not(feature = "web")))
/// skip the redirect ping — there's no client to redirect.
#[cfg(not(feature = "mobile"))]
pub(crate) fn note_server_fn_err(e: dioxus::CapturedError) -> DataError {
    if let Some(dioxus::fullstack::ServerFnError::ServerError { code, message, .. }) =
        e.0.downcast_ref::<dioxus::fullstack::ServerFnError>()
    {
        if *code == 401 {
            #[cfg(feature = "web")]
            web_auth_state::notify_unauthorized();
            return DataError::Unauthorized;
        }
        // Surface the handler's own `ServerFnError::new(msg)` text rather than
        // the `CapturedError` Display, which wraps it as "error running server
        // function: <msg> (details: None)" — noise for an inline form error.
        return DataError::Other(message.clone());
    }
    DataError::Other(e.to_string())
}
