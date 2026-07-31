//! Mobile on-device persistence: the reactive [`ServerUrl`] context plus
//! the app-data-dir resolver and the on-disk server-URL and bearer-token
//! stores. Compiled only with `feature = "mobile"` (gated at the `mod`
//! declaration in [`super`]).

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
