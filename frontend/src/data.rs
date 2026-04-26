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

use omnibus_shared::{EbookLibrary, LibraryContents, Settings};
#[cfg(any(feature = "web", feature = "mobile"))]
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};

// ===== Mobile transport: reqwest =====

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
    //! **TODO (F0.3 follow-up):** in debug builds the token is held in
    //! process memory and persisted to a plaintext file under the user's
    //! home directory. Release builds skip persistence entirely. Replace
    //! with iOS Keychain / Android Keystore via a platform-specific
    //! abstraction before flipping persistence on for release builds.
    //! Tracked in `docs/roadmap/0-3-auth.md`.
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
                        eprintln!("warning: could not persist bearer token: {e}");
                    }
                }
                Op::Delete => {
                    if let Err(e) = delete_token_file(&path) {
                        eprintln!("warning: could not delete bearer token: {e}");
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
fn client_kind() -> &'static str {
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
fn http_client() -> reqwest::Client {
    static CLIENT: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    CLIENT.get_or_init(reqwest::Client::new).clone()
}

#[cfg(feature = "mobile")]
fn with_bearer(rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
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
fn note_status(status: reqwest::StatusCode) -> reqwest::StatusCode {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        token_store::clear();
    }
    status
}

/// Drain the response body and format an error string. Always reading the
/// body — even on the error path — lets reqwest return the underlying TCP
/// connection to its pool instead of dropping it mid-stream, and folds the
/// server's diagnostic text into the user-visible error.
#[cfg(feature = "mobile")]
async fn drain_error(response: reqwest::Response, status: reqwest::StatusCode) -> String {
    let body = response.text().await.unwrap_or_default();
    if body.is_empty() {
        format!("Server error: {status}")
    } else {
        format!("Server error {status}: {body}")
    }
}

#[cfg(feature = "mobile")]
pub async fn get_value(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value");
    let response = with_bearer(http_client().get(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| "missing value field".into())
}

#[cfg(feature = "mobile")]
pub async fn post_increment(server_url: &str) -> Result<i64, String> {
    let url = format!("{server_url}/api/value/increment");
    let response = with_bearer(http_client().post(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    let payload: serde_json::Value = response.json().await.map_err(|e| e.to_string())?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| "missing value field".into())
}

#[cfg(feature = "mobile")]
pub async fn get_settings(server_url: &str) -> Result<Settings, String> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().get(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    response.json::<Settings>().await.map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn save_settings(server_url: &str, settings: Settings) -> Result<Settings, String> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().post(&url).json(&settings))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    response.json::<Settings>().await.map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, String> {
    let url = format!("{server_url}/api/library");
    let response = with_bearer(http_client().get(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    response
        .json::<LibraryContents>()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn get_ebooks(server_url: &str) -> Result<EbookLibrary, String> {
    let url = format!("{server_url}/api/ebooks");
    let response = with_bearer(http_client().get(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    response
        .json::<EbookLibrary>()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(feature = "mobile")]
pub async fn search_ebooks(server_url: &str, q: &str) -> Result<EbookLibrary, String> {
    // Percent-encode the query so FTS5 operators and whitespace survive the
    // URL.
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search?q={encoded}");
    let response = with_bearer(http_client().get(&url))
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    response
        .json::<EbookLibrary>()
        .await
        .map_err(|e| e.to_string())
}

// ===== Mobile auth transport: bearer token =====
//
// Mobile cannot use cookies (Dioxus Native is not a webview), so login
// requests carry `client_kind: "ios"|"android"|"bearer"`, which the server
// uses as the signal to issue a bearer token in the JSON response instead
// of a `Set-Cookie` header. The token is stashed in [`token_store`] and
// attached to every subsequent request via `with_bearer`.

#[cfg(feature = "mobile")]
pub async fn mobile_login(
    server_url: &str,
    username: String,
    password: String,
    device_name: Option<String>,
) -> Result<UserSummary, String> {
    let req = LoginRequest {
        username,
        password,
        client_kind: Some(client_kind().into()),
        device_name,
        client_version: Some(env!("CARGO_PKG_VERSION").into()),
    };
    finish_bearer_auth(post_mobile_auth(server_url, "/api/auth/login", &req).await?)
}

#[cfg(feature = "mobile")]
pub async fn mobile_register(
    server_url: &str,
    username: String,
    password: String,
    device_name: Option<String>,
) -> Result<UserSummary, String> {
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
fn finish_bearer_auth(resp: LoginResponse) -> Result<UserSummary, String> {
    let Some(token) = resp.token else {
        return Err("server did not issue a bearer token".into());
    };
    token_store::set(token);
    Ok(resp.user)
}

#[cfg(feature = "mobile")]
pub async fn mobile_logout(server_url: &str) -> Result<(), String> {
    // Best-effort server revocation, then always clear the local token so a
    // network failure can't leave the device wedged in a "logged in" state.
    let url = format!("{server_url}/api/auth/logout");
    let _ = with_bearer(http_client().post(&url)).send().await;
    token_store::clear();
    Ok(())
}

#[cfg(feature = "mobile")]
async fn post_mobile_auth<T: serde::Serialize>(
    server_url: &str,
    path: &str,
    body: &T,
) -> Result<LoginResponse, String> {
    let url = format!("{server_url}{path}");
    let response = http_client()
        .post(&url)
        .json(body)
        .send()
        .await
        .map_err(|e| format!("{e:#}"))?;
    let status = response.status();
    if !status.is_success() {
        let msg = response.text().await.unwrap_or_default();
        return Err(format!("{status}: {msg}"));
    }
    response
        .json::<LoginResponse>()
        .await
        .map_err(|e| e.to_string())
}

// ===== Web / fullstack-SSR transport: dioxus-fullstack server functions =====
//
// `server_url` is unused here — server functions always resolve against the
// page origin. We keep the parameter so the call sites stay platform-agnostic.

#[cfg(not(feature = "mobile"))]
pub async fn get_value(_server_url: &str) -> Result<i64, String> {
    match crate::rpc::rpc_get_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn post_increment(_server_url: &str) -> Result<i64, String> {
    match crate::rpc::rpc_increment_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, String> {
    crate::rpc::rpc_get_settings()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, String> {
    crate::rpc::rpc_save_settings(settings)
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, String> {
    crate::rpc::rpc_get_library()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks(_server_url: &str) -> Result<EbookLibrary, String> {
    crate::rpc::rpc_get_ebooks()
        .await
        .map_err(|e| e.to_string())
}

#[cfg(not(feature = "mobile"))]
pub async fn search_ebooks(_server_url: &str, q: &str) -> Result<EbookLibrary, String> {
    crate::rpc::rpc_search(q.to_string())
        .await
        .map_err(|e| e.to_string())
}

// ===== Auth transport (web only) =====
//
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

#[cfg(feature = "web")]
pub async fn login(req: LoginRequest) -> Result<LoginResponse, String> {
    post_auth_json("/api/auth/login", &req).await
}

#[cfg(feature = "web")]
pub async fn register(req: RegisterRequest) -> Result<LoginResponse, String> {
    post_auth_json("/api/auth/register", &req).await
}

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
    Ok(())
}

#[cfg(feature = "web")]
pub async fn current_user() -> Result<Option<UserSummary>, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/auth/me")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.status() == 401 {
        return Ok(None);
    }
    if !res.ok() {
        return Err(format!("me failed: {}", res.status()));
    }
    res.json::<UserSummary>()
        .await
        .map(Some)
        .map_err(|e| e.to_string())
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
