//! Feature-gated data-fetching layer: mobile calls the server's
//! hand-written `/api/*` REST routes via `reqwest`; web and server-only
//! compiles call the `#[get]`/`#[post]` functions in [`crate::rpc`].
//! Per-domain wrappers live in the [`auth`], [`authors`], [`books`], and
//! sibling submodules, re-exported here.

mod auth;
mod authors;
mod bookmarks;
mod books;
mod highlights;
mod journals;
mod kindle;
#[cfg(not(feature = "mobile"))]
mod logs;
mod physical;
mod progress;
mod ratings;
mod read_status;
mod scan;
mod series;
mod shelves;
mod stats;
mod suggestions;
mod summary;
mod tags;
mod uploads;
// Admin user management (F5.4) — web (gloo REST) + SSR stubs; no mobile surface.
#[cfg(not(feature = "mobile"))]
mod users;

// auth exports exist under web, mobile, and server-only (the last only
// re-exports the SSR `current_user` stub so pages can call `data::current_user`
// unconditionally without diverging hook order between SSR and WASM).
#[cfg(any(feature = "web", feature = "mobile", feature = "server"))]
pub use auth::*;
pub use authors::*;
pub use bookmarks::*;
pub use books::*;
pub use highlights::*;
pub use journals::*;
pub use kindle::*;
#[cfg(not(feature = "mobile"))]
pub use logs::*;
pub use physical::*;
pub use progress::*;
pub use ratings::*;
pub use read_status::*;
pub use scan::*;
pub use series::*;
pub use shelves::*;
pub use stats::*;
pub use suggestions::*;
pub use summary::*;
pub use tags::*;
pub use uploads::*;
#[cfg(not(feature = "mobile"))]
pub use users::*;

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
    /// Fast-fail result when the client already knows the server is
    /// unreachable (`offline::sync::is_offline()`) and skipped the network
    /// entirely. Unconditional (like [`DataError::Unauthorized`]) so the
    /// enum shape is stable across feature builds.
    #[error("You're offline")]
    Offline,
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
///
/// Reactive: the pre-login Connect screen rewrites this signal, and every
/// `use_server_url()` reader re-renders against the new origin. Provided
/// once by the native shell's `Root`, seeded from [`server_url_store::load`].
#[cfg(feature = "mobile")]
#[derive(Clone, Copy)]
pub struct ServerUrl(pub dioxus::prelude::Signal<String>);

#[cfg(feature = "mobile")]
pub mod app_dirs {
    //! Resolves the writable, per-app directory that [`super::token_store`]
    //! and [`super::server_url_store`] persist into. Historically both wrote
    //! to `$HOME/.omnibus-*`, but on iOS `$HOME` is the app-container *root*,
    //! which is read-only — only its `Library`/`Documents`/`tmp` subdirs are
    //! writable — so every write silently failed and nothing survived a cold
    //! start. We now target `Library/Application Support/omnibus` on iOS (the
    //! Apple-sanctioned, backed-up location for app-managed data),
    //! `$HOME/.omnibus` on desktop/dev, and the app's private files dir
    //! (`Context.getFilesDir()`, resolved via JNI) on Android.
    use std::path::{Path, PathBuf};

    /// Pure per-platform base-dir resolution from a home dir. No I/O, so it's
    /// unit-testable without touching the process environment.
    pub(crate) fn data_dir_from_home(home: &Path) -> PathBuf {
        #[cfg(target_os = "ios")]
        {
            home.join("Library/Application Support/omnibus")
        }
        #[cfg(not(target_os = "ios"))]
        {
            home.join(".omnibus")
        }
    }

    /// Resolve and create the writable data dir. `None` → the caller keeps
    /// its in-memory-only behavior (unchanged from before): no home dir, an
    /// unwritable location, or a failed Android JNI call.
    pub fn data_dir() -> Option<PathBuf> {
        // Android's `$HOME` under tao/wry isn't the app files dir; it's
        // resolved via JNI instead of the home-relative logic below.
        #[cfg(target_os = "android")]
        {
            android_files_dir()
        }
        #[cfg(not(target_os = "android"))]
        {
            let home = std::env::var_os("HOME")?;
            let dir = data_dir_from_home(Path::new(&home));
            // Log before falling back to memory-only: a silent create failure
            // is exactly the class of bug this module exists to fix.
            if let Err(e) = std::fs::create_dir_all(&dir) {
                tracing::warn!(error = %e, path = %dir.display(), "could not create app data dir; persistence disabled this launch");
                return None;
            }
            Some(dir)
        }
    }

    /// Resolve `Context.getFilesDir()` — the app-private, already-writable
    /// internal storage dir Android grants every app — via JNI, using the
    /// ambient `AndroidContext` that tao (dioxus's `mobile` windowing
    /// backend) registers with `ndk-context` before any app code runs. No
    /// `create_dir_all` needed: unlike the home-relative dirs above, Android
    /// guarantees this directory already exists.
    ///
    /// Logs and falls back to memory-only on any JNI failure, mirroring the
    /// `create_dir_all` failure path above — a silent `None` here would be
    /// exactly the class of bug this module exists to fix.
    #[cfg(target_os = "android")]
    fn android_files_dir() -> Option<PathBuf> {
        match android_files_dir_via_jni() {
            Ok(dir) => Some(dir),
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not resolve Android files dir via JNI; persistence disabled this launch"
                );
                None
            }
        }
    }

    #[cfg(target_os = "android")]
    fn android_files_dir_via_jni() -> jni::errors::Result<PathBuf> {
        let ctx = ndk_context::android_context();
        // SAFETY: `ctx.vm()` is the JavaVM pointer `ndk-context` was
        // initialized with at process start (before `main`), so it is valid
        // for the lifetime of the process.
        let vm = unsafe { jni::JavaVM::from_raw(ctx.vm().cast()) }?;
        let mut env = vm.attach_current_thread()?;
        // SAFETY: `ctx.context()` is the `Activity`/`Context` jobject
        // registered alongside the JavaVM above, so it is a valid local JNI
        // reference for the duration of this call.
        let activity = unsafe { jni::objects::JObject::from_raw(ctx.context().cast()) };

        let files_dir = env
            .call_method(&activity, "getFilesDir", "()Ljava/io/File;", &[])?
            .l()?;
        let path_obj = env
            .call_method(&files_dir, "getAbsolutePath", "()Ljava/lang/String;", &[])?
            .l()?;
        let path: String = env
            .get_string(&jni::objects::JString::from(path_obj))?
            .into();
        Ok(PathBuf::from(path))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn data_dir_from_home_targets_platform_subdir() {
            let dir = data_dir_from_home(Path::new("/home/reader"));
            if cfg!(target_os = "ios") {
                assert_eq!(
                    dir,
                    Path::new("/home/reader/Library/Application Support/omnibus")
                );
            } else {
                assert_eq!(dir, Path::new("/home/reader/.omnibus"));
            }
        }
    }
}

#[cfg(feature = "mobile")]
pub mod server_url_store {
    //! On-disk persistence for the user-entered backend base URL. Far
    //! simpler than [`super::token_store`]: the URL is not a secret, so it
    //! persists with default permissions and no in-memory cache or change
    //! channel — the reactive [`super::ServerUrl`] context signal is the
    //! in-memory source of truth, while this module only reads it once at
    //! launch and writes it back when the user connects. Persistence is
    //! conditional on a writable dir (see [`super::app_dirs::data_dir`]),
    //! available on iOS/desktop/Android; a `None` (e.g. a failed Android JNI
    //! call) falls back to memory-only for that launch.
    use std::path::PathBuf;

    /// On-disk path for the persisted server URL, or `None` when no writable
    /// app data dir is available (see [`super::app_dirs::data_dir`]).
    pub fn server_path() -> Option<PathBuf> {
        super::app_dirs::data_dir().map(|d| d.join("server"))
    }

    /// Trim persisted file contents into a usable URL, rejecting an
    /// empty/whitespace-only file (treated as "nothing saved").
    fn parse_loaded(raw: &str) -> Option<String> {
        let trimmed = raw.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_string())
    }

    /// Read the persisted server URL, if any. Errors (missing/unreadable
    /// file) are swallowed to `None` — the user just re-enters the URL.
    pub fn load() -> Option<String> {
        let path = server_path()?;
        let contents = std::fs::read_to_string(&path).ok()?;
        parse_loaded(&contents)
    }

    /// Persist the server URL, best-effort. A write failure is logged and
    /// ignored: the in-memory context signal still carries the value for
    /// this session, so the only cost is re-entering it next launch.
    pub fn set(url: &str) {
        let Some(path) = server_path() else { return };
        if let Err(e) = std::fs::write(&path, url) {
            tracing::warn!(error = %e, path = %path.display(), "could not persist server URL");
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parse_loaded_trims_and_returns_some_for_a_url() {
            assert_eq!(
                parse_loaded("  https://omnibus.local:3000\n"),
                Some("https://omnibus.local:3000".to_string())
            );
        }

        #[test]
        fn parse_loaded_returns_none_for_empty_or_whitespace() {
            assert_eq!(parse_loaded(""), None);
            assert_eq!(parse_loaded("   \n\t"), None);
        }
    }
}

#[cfg(feature = "mobile")]
pub mod token_store {
    //! In-process bearer-token store for the mobile client. In-memory state
    //! lives in an `RwLock<Option<String>>` (see [`unpoison`]); disk
    //! persistence is funnelled through a single dedicated worker thread fed
    //! by an `mpsc` channel (see [`persistence_tx`]) so `set` / `clear`
    //! never block on flash I/O and can't race each other.
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
    /// Returns `None` when no writable app data dir is available (see
    /// [`super::app_dirs::data_dir`]); the token then stays in memory only and
    /// the user re-logs in on next launch — strictly safer than dropping a
    /// token file in an arbitrary or unwritable location.
    pub fn token_path() -> Option<PathBuf> {
        super::app_dirs::data_dir().map(|d| d.join("token"))
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

    /// `true` when this build persists the bearer token to disk. No longer
    /// gated on `debug_assertions` — release builds persist too, so users stay
    /// signed in across a cold start and are only logged out when the server
    /// rejects the bearer (expiry/revoke). The actual write still depends on a
    /// writable dir: on iOS/desktop/Android the token lands in the sandboxed
    /// app data dir (see [`super::app_dirs`]) with `0o600` perms (protected at
    /// rest by the iOS sandbox + Data Protection, or Android's per-app UID
    /// sandbox). Neither is encryption-at-rest; iOS Keychain / Android
    /// Keystore remains a future hardening step.
    fn persistence_enabled() -> bool {
        true
    }

    /// Set the token in memory immediately, notify UI subscribers, and (when
    /// a writable [`token_path`] exists) enqueue a disk write on the
    /// persistence worker.
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

    /// Clear the token from memory immediately, notify UI subscribers, and
    /// (when a writable [`token_path`] exists) enqueue a disk delete on the
    /// persistence worker. Channel ordering guarantees a clear always
    /// supersedes any earlier set.
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

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn persistence_enabled_is_true_in_every_build() {
            // Release builds must persist too — a false here would resurrect
            // the "logged out on app close" bug.
            assert!(persistence_enabled());
        }

        #[test]
        fn write_token_file_round_trips_the_token() {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("token");
            write_token_file(&path, b"secret-bearer").expect("write");
            assert_eq!(std::fs::read_to_string(&path).unwrap(), "secret-bearer");
        }

        #[cfg(unix)]
        #[test]
        fn write_token_file_is_owner_only_on_unix() {
            use std::os::unix::fs::PermissionsExt;
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("token");
            // Pre-create with loose perms to prove the mode is re-applied even
            // when the file already exists (the buggy-older-build case).
            std::fs::write(&path, b"old").unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            write_token_file(&path, b"secret-bearer").expect("write");
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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

    // `Decode` only exists on the web/mobile builds that link `serde_json`
    // for direct body deserialization (see the variant's doc comment); gated
    // to match so this test simply doesn't compile into a server-only build.
    #[cfg(any(feature = "mobile", feature = "web"))]
    #[test]
    fn decode_wraps_a_malformed_json_body_parse_failure() {
        // A truncated/invalid body run through the same `serde_json`
        // deserializer the web/SSR path uses, surfaced through `#[from]` as
        // `DataError::Decode`.
        let src = serde_json::from_str::<serde_json::Value>("{not valid json").unwrap_err();
        let err: DataError = src.into();
        assert!(matches!(err, DataError::Decode(_)), "got {err:?}");
        assert!(
            err.to_string()
                .starts_with("response deserialization failed"),
            "got {err}"
        );
    }
}
