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

use omnibus_shared::{
    AuthorDetail, AuthorPhotoScanResult, AuthorSummary, EbookLibrary, EbookMetadata,
    LibraryContents, MetadataOverrides, PaletteResults, SeriesDetail, SeriesSummary, Settings,
    TagWeight, WorkerStatus,
};
#[cfg(any(feature = "web", feature = "mobile"))]
use omnibus_shared::{LoginRequest, LoginResponse, RegisterRequest, UserSummary};

// ===== Typed transport error (#96) =====
//
// Replaces the previous `Result<T, String>` everywhere in this module so
// callers can distinguish failure modes by type — most importantly
// `Unauthorized`, which the mobile 401 handler and the web router both key
// on. The variants that carry a foreign error type (`reqwest`, `serde_json`)
// are feature-gated to match the optional deps that provide them: `reqwest`
// is mobile-only, `serde_json` is web+mobile. `Unauthorized`, `Http`, and
// the `Other` catch-all are always present so the enum's public shape is
// stable across every build that compiles the callers.

/// Errors surfaced by the feature-gated data transport.
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
async fn drain_error(response: reqwest::Response, status: reqwest::StatusCode) -> DataError {
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return DataError::Unauthorized;
    }
    let body = response.text().await.unwrap_or_default();
    DataError::Http {
        status: status.as_u16(),
        body,
    }
}

#[cfg(feature = "mobile")]
pub async fn get_value(server_url: &str) -> Result<i64, DataError> {
    let url = format!("{server_url}/api/value");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    let payload: serde_json::Value = response.json().await?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| DataError::Other("missing value field".into()))
}

#[cfg(feature = "mobile")]
pub async fn post_increment(server_url: &str) -> Result<i64, DataError> {
    let url = format!("{server_url}/api/value/increment");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    let payload: serde_json::Value = response.json().await?;
    payload["value"]
        .as_i64()
        .ok_or_else(|| DataError::Other("missing value field".into()))
}

#[cfg(feature = "mobile")]
pub async fn get_settings(server_url: &str) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

#[cfg(feature = "mobile")]
pub async fn save_settings(server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    let url = format!("{server_url}/api/settings");
    let response = with_bearer(http_client().post(&url).json(&settings))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Settings>().await?)
}

#[cfg(feature = "mobile")]
pub async fn get_library(server_url: &str) -> Result<LibraryContents, DataError> {
    let url = format!("{server_url}/api/library");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<LibraryContents>().await?)
}

#[cfg(feature = "mobile")]
pub async fn get_ebooks(server_url: &str) -> Result<EbookLibrary, DataError> {
    let url = format!("{server_url}/api/ebooks");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

#[cfg(feature = "mobile")]
pub async fn search_ebooks(server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
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
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<EbookLibrary>().await?)
}

/// Search palette — grouped results for the command-palette overlay (F1.5).
#[cfg(feature = "mobile")]
pub async fn search_palette(server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    let encoded: String = q
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect();
    let url = format!("{server_url}/api/search/palette?q={encoded}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<PaletteResults>().await?)
}

#[cfg(feature = "mobile")]
pub async fn get_ebook(server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

#[cfg(feature = "mobile")]
pub async fn get_author(server_url: &str, id: i64) -> Result<Option<AuthorDetail>, DataError> {
    let url = format!("{server_url}/api/authors/{id}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<AuthorDetail>().await?))
}

/// F1.11 follow-up: persist an author photo by URL. Server fetches and
/// validates the URL — see `db::author_photos::fetch_remote_image`.
#[cfg(feature = "mobile")]
pub async fn set_author_photo_url(server_url: &str, id: i64, url: String) -> Result<(), DataError> {
    let endpoint = format!("{server_url}/api/authors/{id}/photo/url");
    let response = with_bearer(http_client().put(&endpoint))
        .json(&serde_json::json!({ "url": url }))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// F1.11 follow-up: multipart upload of an author photo. Mobile mirrors the
/// web FormData path — the same `/api/authors/:id/photo` PUT endpoint.
#[cfg(feature = "mobile")]
pub async fn upload_author_photo(
    server_url: &str,
    id: i64,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
    let endpoint = format!("{server_url}/api/authors/{id}/photo");
    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name(filename)
        .mime_str(&mime)?;
    let form = reqwest::multipart::Form::new().part("photo", part);
    let response = with_bearer(http_client().put(&endpoint))
        .multipart(form)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// F1.11: admin "Scan for picture" — synchronously re-runs the Open Library
/// cascade and returns whether a photo was found.
#[cfg(feature = "mobile")]
pub async fn scan_author_photo(
    server_url: &str,
    id: i64,
) -> Result<AuthorPhotoScanResult, DataError> {
    let url = format!("{server_url}/api/authors/{id}/photo/scan");
    let response = with_bearer(http_client().post(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<AuthorPhotoScanResult>().await?)
}

#[cfg(feature = "mobile")]
pub async fn get_series(server_url: &str, id: i64) -> Result<Option<SeriesDetail>, DataError> {
    let url = format!("{server_url}/api/series/{id}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if status == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<SeriesDetail>().await?))
}

#[cfg(feature = "mobile")]
pub async fn list_authors(server_url: &str) -> Result<Vec<AuthorSummary>, DataError> {
    let url = format!("{server_url}/api/authors");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<AuthorSummary>>().await?)
}

#[cfg(feature = "mobile")]
pub async fn list_series(server_url: &str) -> Result<Vec<SeriesSummary>, DataError> {
    let url = format!("{server_url}/api/series");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<SeriesSummary>>().await?)
}

#[cfg(feature = "mobile")]
pub async fn get_tag_cloud(server_url: &str) -> Result<Vec<TagWeight>, DataError> {
    let url = format!("{server_url}/api/tags");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<TagWeight>>().await?)
}

#[cfg(feature = "mobile")]
pub async fn save_overrides(
    server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().post(&url))
        .json(overrides)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
}

#[cfg(feature = "mobile")]
pub async fn delete_overrides(
    server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/overrides");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(Some(response.json::<EbookMetadata>().await?))
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
) -> Result<UserSummary, DataError> {
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
) -> Result<UserSummary, DataError> {
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

#[cfg(feature = "mobile")]
pub async fn mobile_logout(server_url: &str) -> Result<(), DataError> {
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

// ===== Web auth state =====
//
// #57: web counterpart to `token_store::subscribe()`. The web client uses
// session cookies (round-tripped automatically by the browser), so there's
// no client-side token to clear — but we still need a reactive signal so
// the router can redirect to /login when any data-layer call returns 401
// (session expired, server restarted, admin revoked). All web data
// wrappers route their errors through [`note_server_fn_err`] below, which
// pushes `false` onto this channel on a 401 response; `ScreenLayout`
// subscribes and `nav.replace`s.

#[cfg(feature = "web")]
pub mod web_auth_state {
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
fn note_server_fn_err(e: dioxus::CapturedError) -> DataError {
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

// ===== Web / fullstack-SSR transport: dioxus-fullstack server functions =====
//
// `server_url` is unused here — server functions always resolve against the
// page origin. We keep the parameter so the call sites stay platform-agnostic.

#[cfg(not(feature = "mobile"))]
pub async fn get_value(_server_url: &str) -> Result<i64, DataError> {
    match crate::rpc::rpc_get_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(note_server_fn_err(e)),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn post_increment(_server_url: &str) -> Result<i64, DataError> {
    match crate::rpc::rpc_increment_value().await {
        Ok(payload) => Ok(payload.value),
        Err(e) => Err(note_server_fn_err(e)),
    }
}

#[cfg(not(feature = "mobile"))]
pub async fn get_settings(_server_url: &str) -> Result<Settings, DataError> {
    crate::rpc::rpc_get_settings()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn save_settings(_server_url: &str, settings: Settings) -> Result<Settings, DataError> {
    crate::rpc::rpc_save_settings(settings)
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn get_library(_server_url: &str) -> Result<LibraryContents, DataError> {
    crate::rpc::rpc_get_library()
        .await
        .map_err(note_server_fn_err)
}

/// Snapshot of the worker progress feed. Web calls the RPC; mobile returns
/// an empty status because the corresponding REST endpoint doesn't exist
/// yet (issue #69 keeps the mobile UI out of scope for v1, and the data
/// stub keeps callers' types lined up across feature gates).
#[cfg(not(feature = "mobile"))]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    crate::rpc::rpc_worker_status()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(feature = "mobile")]
pub async fn worker_status(_server_url: &str) -> Result<WorkerStatus, DataError> {
    // Mobile REST mirror is a follow-up; return an empty status so any
    // future mobile caller compiles against the same signature the web
    // build uses.
    Ok(WorkerStatus::default())
}

#[cfg(not(feature = "mobile"))]
pub async fn get_ebooks(_server_url: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_get_ebooks()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn search_ebooks(_server_url: &str, q: &str) -> Result<EbookLibrary, DataError> {
    crate::rpc::rpc_search(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Search palette — grouped results for the command-palette overlay (F1.5).
#[cfg(not(feature = "mobile"))]
pub async fn search_palette(_server_url: &str, q: &str) -> Result<PaletteResults, DataError> {
    crate::rpc::rpc_search_palette(q.to_string())
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn get_ebook(_server_url: &str, uuid: &str) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_get_ebook(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn get_author(_server_url: &str, id: i64) -> Result<Option<AuthorDetail>, DataError> {
    crate::rpc::rpc_get_author(id)
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn scan_author_photo(
    _server_url: &str,
    id: i64,
) -> Result<AuthorPhotoScanResult, DataError> {
    crate::rpc::rpc_scan_author_photo(id)
        .await
        .map_err(note_server_fn_err)
}

/// F5.9-lite: admin "Delete author" (issue #159). Removes the author
/// taxonomy row, drops every `books_authors_link` for it, and adds the
/// name to `ignored_authors` so the next `indexer::reindex` does not
/// silently resurrect the row. Returns the number of books that were
/// un-linked (used by the confirmation modal's "this affects N books"
/// copy). Web-only — mobile parity is a deliberate follow-up per the
/// F5.9-lite plan.
#[cfg(not(feature = "mobile"))]
pub async fn delete_author(_server_url: &str, id: i64) -> Result<u64, DataError> {
    crate::rpc::rpc_delete_author(id)
        .await
        .map_err(note_server_fn_err)
}

/// F1.11 follow-up: persist an author photo by URL. Web routes through the
/// `#[post]` server function in `rpc.rs`, which performs the server-side
/// fetch + validation and writes a `manual` row.
#[cfg(not(feature = "mobile"))]
pub async fn set_author_photo_url(
    _server_url: &str,
    id: i64,
    url: String,
) -> Result<(), DataError> {
    crate::rpc::rpc_set_author_photo_url(id, url)
        .await
        .map_err(note_server_fn_err)
}

/// F1.11 follow-up: multipart upload of an author photo on the web client.
///
/// Server functions can't carry binary file uploads (they JSON-serialize
/// their arguments), so this bypasses RPC and POSTs the bytes directly to
/// the REST endpoint via `gloo-net`. The browser auto-attaches the
/// `omnibus_session` cookie on a same-origin request, so no manual auth
/// plumbing is needed. SSR doesn't call this — it only fires from a user
/// `onchange` handler after hydration.
#[cfg(feature = "web")]
pub async fn upload_author_photo(
    _server_url: &str,
    id: i64,
    filename: String,
    mime: String,
    bytes: Vec<u8>,
) -> Result<(), DataError> {
    use gloo_net::http::Request;
    use wasm_bindgen::JsCast;

    let endpoint = format!("/api/authors/{id}/photo");
    let form =
        web_sys::FormData::new().map_err(|e| DataError::Other(format!("FormData::new: {e:?}")))?;
    // `Blob` ctor wants a `&Array` of `BufferSource | BlobPart` parts —
    // build a one-element Uint8Array, drop it into a JS Array, then hand
    // that to `Blob::new_with_u8_array_sequence_and_options`.
    let u8 = js_sys::Uint8Array::from(bytes.as_slice());
    let parts = js_sys::Array::new();
    parts.push(&u8);
    let mut opts = web_sys::BlobPropertyBag::new();
    opts.type_(&mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &opts)
        .map_err(|e| DataError::Other(format!("Blob::new: {e:?}")))?;
    form.append_with_blob_and_filename("photo", &blob, &filename)
        .map_err(|e| DataError::Other(format!("FormData::append: {e:?}")))?;

    let res = Request::put(&endpoint)
        // gloo-net's `body` takes anything `Into<JsValue>` — FormData
        // satisfies that via its JsCast impl. Don't set Content-Type:
        // the browser fills it in with the multipart boundary.
        .body(form.unchecked_into::<wasm_bindgen::JsValue>())
        .map_err(|e| DataError::Other(e.to_string()))?
        .send()
        .await
        .map_err(|e| DataError::Other(e.to_string()))?;
    if res.status() == 401 {
        web_auth_state::notify_unauthorized();
        return Err(DataError::Unauthorized);
    }
    if !res.ok() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        return Err(DataError::Http { status, body });
    }
    Ok(())
}

/// Fallback stub for the non-web, non-mobile build (cargo check on the
/// default workspace members compiles the frontend with no platform
/// feature so type-checking still passes). The author detail page only
/// invokes upload after a user `onchange`, which never fires under SSR.
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub async fn upload_author_photo(
    _server_url: &str,
    _id: i64,
    _filename: String,
    _mime: String,
    _bytes: Vec<u8>,
) -> Result<(), DataError> {
    Err(DataError::Other(
        "upload not available in this build".into(),
    ))
}

#[cfg(not(feature = "mobile"))]
pub async fn get_series(_server_url: &str, id: i64) -> Result<Option<SeriesDetail>, DataError> {
    crate::rpc::rpc_get_series(id)
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn get_tag_cloud(_server_url: &str) -> Result<Vec<TagWeight>, DataError> {
    crate::rpc::rpc_get_tag_cloud()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn list_authors(_server_url: &str) -> Result<Vec<AuthorSummary>, DataError> {
    crate::rpc::rpc_list_authors()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn list_series(_server_url: &str) -> Result<Vec<SeriesSummary>, DataError> {
    crate::rpc::rpc_list_series()
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn save_overrides(
    _server_url: &str,
    uuid: &str,
    overrides: &MetadataOverrides,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_save_overrides(uuid.to_string(), overrides.clone())
        .await
        .map_err(note_server_fn_err)
}

#[cfg(not(feature = "mobile"))]
pub async fn delete_overrides(
    _server_url: &str,
    uuid: &str,
) -> Result<Option<EbookMetadata>, DataError> {
    crate::rpc::rpc_delete_overrides(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
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
    let resp = post_auth_json("/api/auth/login", &req).await?;
    // Reset the auth-state channel so a prior 401 doesn't keep
    // ScreenLayout in redirect mode after a successful re-login.
    web_auth_state::notify_authorized();
    Ok(resp)
}

#[cfg(feature = "web")]
pub async fn register(req: RegisterRequest) -> Result<LoginResponse, String> {
    let resp = post_auth_json("/api/auth/register", &req).await?;
    web_auth_state::notify_authorized();
    Ok(resp)
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
    // A successful logout is the unauth path — same signal as a 401, so
    // ScreenLayout redirects to /login without each caller having to nav.
    web_auth_state::notify_unauthorized();
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
        web_auth_state::notify_unauthorized();
        return Ok(None);
    }
    if !res.ok() {
        return Err(format!("me failed: {}", res.status()));
    }
    let user = res.json::<UserSummary>().await.map_err(|e| e.to_string())?;
    web_auth_state::notify_authorized();
    Ok(Some(user))
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
