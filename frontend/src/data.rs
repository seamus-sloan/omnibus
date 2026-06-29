//! Feature-gated data-fetching layer.
//!
//! - Mobile (`feature = "mobile"`) calls the server's hand-written REST
//!   routes (`/api/*`) via `reqwest`, picking up the base URL from the
//!   `ServerUrl` Dioxus context. `server_url` is required at the call site.
//! - Web (`feature = "web"`) calls the `#[get]`/`#[post]` server functions
//!   defined in [`crate::rpc`]. No base URL needed — the server-function
//!   client stubs use the page origin automatically. `server_url` is
//!   ignored on the web path.
//! - Server-only compiles (`feature = "server"` without `"web"`) reuse the
//!   web stubs so SSR-during-fullstack-render still returns sensible data.
//!
//! Per-domain wrappers live in the [`auth`], [`authors`], [`books`],
//! [`highlights`], [`progress`], [`series`], and [`tags`] submodules and
//! are re-exported here so callers keep importing through
//! `omnibus_frontend::data::*`.

mod auth;
mod authors;
mod bookmarks;
mod books;
mod highlights;
mod progress;
mod series;
mod tags;

// auth exports exist under web, mobile, and server-only (the last only
// re-exports the SSR `current_user` stub so pages can call `data::current_user`
// unconditionally without diverging hook order between SSR and WASM).
#[cfg(any(feature = "web", feature = "mobile", feature = "server"))]
pub use auth::*;
pub use authors::*;
pub use bookmarks::*;
pub use books::*;
pub use highlights::*;
pub use progress::*;
pub use series::*;
pub use tags::*;

/// Errors surfaced by the feature-gated data transport.
///
/// Replaces the previous `Result<T, String>` so callers can distinguish
/// failure modes by type — most importantly `Unauthorized`, which the
/// mobile 401 handler and the web router both key on. The variants that
/// carry a foreign error type (`reqwest`, `serde_json`) are feature-gated
/// to match the optional deps that provide them: `reqwest` is mobile-only,
/// `serde_json` is web+mobile. `Unauthorized`, `Http`, and the `Other`
/// catch-all are always present so the enum's public shape is stable
/// across every build that compiles the callers.
#[derive(Debug, thiserror::Error)]
pub enum DataError {
    /// `reqwest`-level failure on the mobile transport: connect / timeout /
    /// TLS **and** response-body decode errors. The mobile calls deserialize
    /// via `response.json()`, which surfaces a malformed body as a
    /// `reqwest::Error` (`reqwest::Error::is_decode()`), not a
    /// `serde_json::Error` — so a decode failure on mobile lands here rather
    /// than in [`DataError::Decode`]. That `Decode` variant is produced only
    /// by the web/SSR path, which deserializes through `serde_json` directly.
    /// Mobile-only because `reqwest` is only linked under `feature = "mobile"`.
    #[cfg(feature = "mobile")]
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    /// The server responded with a non-success status (other than 401, which
    /// maps to [`DataError::Unauthorized`]). `body` carries the server's
    /// diagnostic text so callers that surface it — e.g. the register-error
    /// classifier — keep working.
    #[error("server returned {status}")]
    Http { status: u16, body: String },
    /// Response body could not be deserialized into the expected type.
    #[cfg(any(feature = "mobile", feature = "web"))]
    #[error("response deserialization failed: {0}")]
    Decode(#[from] serde_json::Error),
    /// Authentication failed (HTTP 401). Distinct variant so the 401 →
    /// clear-token → redirect-to-/login flow can pattern-match instead of
    /// re-inspecting a raw status code.
    #[error("unauthorized")]
    Unauthorized,
    /// Catch-all for transport paths that don't carry a typed source —
    /// the web server-function client (whose error is already stringified
    /// by `note_server_fn_err`), the `gloo-net` web/SSR stubs, and a couple
    /// of protocol invariants (missing JSON field, absent bearer token).
    #[error("{0}")]
    Other(String),
}

impl DataError {
    /// `true` when this represents an authentication failure. Lets callers
    /// branch on auth without depending on a specific HTTP code.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, DataError::Unauthorized)
    }
}

/// Dioxus context wrapper holding the backend base URL for mobile clients.
#[cfg(feature = "mobile")]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerUrl(pub String);

#[cfg(feature = "mobile")]
pub mod token_store {
    //! In-process bearer-token store for the mobile client.
    //!
    //! Threading model:
    //!
    //! * In-memory state lives in an `RwLock<Option<String>>`. Reads/writes
    //!   recover from poisoned locks via [`unpoison`] so a panic in one
    //!   thread can't brick the whole app.
    //! * Disk persistence is funnelled through a single dedicated worker
    //!   thread fed by an `mpsc` channel. This serializes `set` and
    //!   `clear` operations — a delayed write can never overtake a later
    //!   clear and resurrect the token on next launch. **Persistence
    //!   only runs in debug builds** (gated by `persistence_enabled()`,
    //!   which returns `cfg!(debug_assertions)`) so a release build can
    //!   never accidentally drop a long-lived credential on the
    //!   filesystem in plaintext. Release users re-login on every cold
    //!   start until secure storage lands.
    //! * `set` and `clear` update the in-memory cell synchronously and
    //!   enqueue the disk op. Async callers (`mobile_login`,
    //!   `mobile_register`, the 401 handler in `note_status`) never block
    //!   on flash I/O.
    //!
    //! **TODO:** in debug builds the token is held in process memory and
    //! persisted to a plaintext file under the user's home directory.
    //! Release builds skip persistence entirely. Replace with iOS Keychain /
    //! Android Keystore via a platform-specific abstraction before flipping
    //! persistence on for release builds.
    use std::path::{Path, PathBuf};
    use std::sync::{mpsc, LockResult, Mutex, OnceLock, RwLock};
    use tokio::sync::watch;

    enum Op {
        Write(String),
        Delete,
    }

    fn cell() -> &'static RwLock<Option<String>> {
        static CELL: OnceLock<RwLock<Option<String>>> = OnceLock::new();
        CELL.get_or_init(|| RwLock::new(None))
    }

    /// Single broadcast channel that tells UI components when the
    /// authenticated state changes. `Sender::send` is a sync, allocation-
    /// free signal — callable from any thread, with or without an active
    /// async runtime — so `set` / `clear` / `load_from_disk` can all push
    /// updates uniformly. Components subscribe via [`subscribe`] and react
    /// inside a `use_future` loop.
    fn channel() -> &'static (watch::Sender<bool>, watch::Receiver<bool>) {
        static CH: OnceLock<(watch::Sender<bool>, watch::Receiver<bool>)> = OnceLock::new();
        CH.get_or_init(|| watch::channel(false))
    }

    /// Get a fresh receiver tracking whether a token is currently set.
    /// Initial value reflects the state at subscribe time.
    pub fn subscribe() -> watch::Receiver<bool> {
        channel().0.subscribe()
    }

    fn notify(authed: bool) {
        // `send_replace` doesn't require active receivers and never errors,
        // so it's safe from any context.
        channel().0.send_replace(authed);
    }

    /// Recover from a poisoned lock instead of panicking. The token store
    /// is best-effort by design; if some background thread panicked while
    /// holding the lock the worst-case behavior is "user is treated as
    /// logged out and re-prompts," which is much better than crashing the
    /// app.
    fn unpoison<T>(r: LockResult<T>) -> T {
        r.unwrap_or_else(|e| e.into_inner())
    }

    /// On-disk path for the persisted bearer token.
    ///
    /// Returns `None` when no platform-appropriate home directory is
    /// available (`HOME` unset on a non-Unix-y target, etc.). In that case
    /// the token stays in memory only and the user re-logs in on next
    /// launch — strictly safer than dropping a token file in an arbitrary
    /// working directory. iOS app sandboxes set `HOME` to the app's
    /// container, so the common path is covered.
    pub fn token_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".omnibus-token"))
    }

    /// Read the on-disk token (if any) into the in-memory store. Call once
    /// at app launch. Errors are swallowed: a missing or unreadable file
    /// just means the user must log in again.
    pub fn load_from_disk() {
        if !persistence_enabled() {
            return;
        }
        let Some(path) = token_path() else { return };
        if let Ok(s) = std::fs::read_to_string(&path) {
            let trimmed = s.trim().to_string();
            if !trimmed.is_empty() {
                // Tighten perms best-effort on Unix in case an older build
                // wrote the file with the default umask. We can't undo a
                // disclosure that already happened, but we can stop it
                // continuing every launch from now on.
                #[cfg(unix)]
                {
                    use std::fs::Permissions;
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(&path, Permissions::from_mode(0o600));
                }
                *unpoison(cell().write()) = Some(trimmed);
                notify(true);
            }
        }
    }

    /// Snapshot of the current bearer token, if logged in.
    pub fn get() -> Option<String> {
        unpoison(cell().read()).clone()
    }

    /// `true` when this build is allowed to persist the bearer token to
    /// disk. Gated behind `cfg(debug_assertions)` so a release build can
    /// never accidentally write a long-lived credential to the
    /// filesystem in plaintext — release users re-login on every cold
    /// start until iOS Keychain / Android Keystore support lands and
    /// flips this to unconditionally `true` (against secure storage).
    /// Dev builds (`dx serve --platform ios|android`) keep the
    /// persistence path so the dev-loop UX isn't crippled.
    fn persistence_enabled() -> bool {
        cfg!(debug_assertions)
    }

    /// Set the token in memory immediately, notify UI subscribers, and
    /// (in dev builds only) enqueue a disk write on the persistence
    /// worker.
    pub fn set(token: String) {
        *unpoison(cell().write()) = Some(token.clone());
        notify(true);
        if !persistence_enabled() {
            return;
        }
        if let Some(tx) = persistence_tx() {
            let _ = tx.send(Op::Write(token));
        }
    }

    /// Clear the token from memory immediately, notify UI subscribers,
    /// and (in dev builds only) enqueue a disk delete on the persistence
    /// worker. Channel ordering guarantees a clear always supersedes any
    /// earlier set.
    pub fn clear() {
        *unpoison(cell().write()) = None;
        notify(false);
        if !persistence_enabled() {
            return;
        }
        if let Some(tx) = persistence_tx() {
            let _ = tx.send(Op::Delete);
        }
    }

    /// Cached state of the persistence worker. Once we've decided
    /// persistence isn't possible (no `HOME`, thread spawn failed) we
    /// record `Disabled` and never re-attempt — otherwise every
    /// `set`/`clear` would re-run `token_path()` and `Builder::spawn`.
    enum TxState {
        Disabled,
        Ready(mpsc::Sender<Op>),
    }

    /// Lazily start the persistence worker on first use and return a
    /// sender to its op channel. Returns `None` if either the worker
    /// thread fails to spawn or there is no on-disk path to persist to;
    /// callers in those cases simply skip persistence and the in-memory
    /// state remains authoritative. The decision is cached in `SLOT`
    /// so that follow-up calls don't re-run the spawn dance.
    fn persistence_tx() -> Option<mpsc::Sender<Op>> {
        static SLOT: OnceLock<Mutex<Option<TxState>>> = OnceLock::new();
        let slot = SLOT.get_or_init(|| Mutex::new(None));
        let mut guard = unpoison(slot.lock());
        if let Some(state) = guard.as_ref() {
            return match state {
                TxState::Disabled => None,
                TxState::Ready(tx) => Some(tx.clone()),
            };
        }
        let Some(path) = token_path() else {
            *guard = Some(TxState::Disabled);
            return None;
        };
        let (tx, rx) = mpsc::channel::<Op>();
        if std::thread::Builder::new()
            .name("omnibus-token-store".into())
            .spawn(move || persistence_worker(path, rx))
            .is_err()
        {
            *guard = Some(TxState::Disabled);
            return None;
        }
        *guard = Some(TxState::Ready(tx.clone()));
        Some(tx)
    }

    fn persistence_worker(path: PathBuf, rx: mpsc::Receiver<Op>) {
        while let Ok(op) = rx.recv() {
            match op {
                Op::Write(token) => {
                    if let Err(e) = write_token_file(&path, token.as_bytes()) {
                        tracing::warn!(
                            error = %e,
                            path = %path.display(),
                            "could not persist bearer token"
                        );
                    }
                }
                Op::Delete => {
                    if let Err(e) = delete_token_file(&path) {
                        tracing::warn!(
                            error = %e,
                            path = %path.display(),
                            "could not delete bearer token"
                        );
                    }
                }
            }
        }
    }

    /// Remove the on-disk token, falling back to overwriting with an
    /// empty file when `remove_file` fails (e.g. a permissions glitch on
    /// the parent dir, or a sandboxed filesystem that allows write but
    /// not unlink). Without the fallback a failed unlink would silently
    /// keep the user logged in across the next launch — `load_from_disk`
    /// short-circuits on empty content, so an empty file is functionally
    /// equivalent to an absent one.
    fn delete_token_file(path: &Path) -> std::io::Result<()> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(remove_err) => match write_token_file(path, b"") {
                Ok(()) => Ok(()),
                Err(_) => Err(remove_err),
            },
        }
    }

    /// Write the token with owner-only permissions on Unix so other local
    /// users on a shared machine can't read it. The mode is re-applied
    /// after every write because `OpenOptions::mode` only takes effect on
    /// initial creation — a pre-existing file with looser perms (e.g.
    /// from a buggy older build) would otherwise stay readable.
    #[cfg(unix)]
    fn write_token_file(path: &Path, token: &[u8]) -> std::io::Result<()> {
        use std::fs::{OpenOptions, Permissions};
        use std::io::Write;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

        let mut f = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(path)?;
        f.write_all(token)?;
        std::fs::set_permissions(path, Permissions::from_mode(0o600))
    }

    #[cfg(not(unix))]
    fn write_token_file(path: &Path, token: &[u8]) -> std::io::Result<()> {
        std::fs::write(path, token)
    }
}

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
    CLIENT.get_or_init(reqwest::Client::new).clone()
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
    if let Some(sfn_err) = e.0.downcast_ref::<dioxus::fullstack::ServerFnError>() {
        let code = match sfn_err {
            dioxus::fullstack::ServerFnError::ServerError { code, .. } => *code,
            _ => 0,
        };
        if code == 401 {
            #[cfg(feature = "web")]
            web_auth_state::notify_unauthorized();
            return DataError::Unauthorized;
        }
    }
    DataError::Other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unauthorized_reports_is_unauthorized() {
        assert!(DataError::Unauthorized.is_unauthorized());
        assert_eq!(DataError::Unauthorized.to_string(), "unauthorized");
    }

    #[test]
    fn http_carries_status_and_is_not_unauthorized() {
        let err = DataError::Http {
            status: 400,
            body: "bad request".into(),
        };
        assert!(!err.is_unauthorized());
        // Display intentionally omits the body — callers that need it match
        // on the `Http { body, .. }` variant directly.
        assert_eq!(err.to_string(), "server returned 400");
    }

    #[test]
    fn other_round_trips_its_message() {
        let err = DataError::Other("missing value field".into());
        assert!(!err.is_unauthorized());
        assert_eq!(err.to_string(), "missing value field");
    }
}
