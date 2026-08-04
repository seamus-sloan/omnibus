//! Process-start metadata surfaced by `GET /api/_health`: a build
//! fingerprint, the workspace root the server was launched from, and the
//! running release version. Each is captured once (via `OnceLock`) and
//! stable for the process lifetime. Split out of `backend.rs` (#1672).

/// Process-start build id. Captured once and preserved for the lifetime of
/// the process — so any HMR cycle that restarts the server (the only way
/// `dx serve` rebuilds Rust changes) produces a new id. Claude's
/// `ui-validate` skill polls this to know when a rebuild has actually
/// landed.
///
/// `main.rs` calls [`init_build_id`] eagerly during boot so the id is set
/// before any request can read it; this keeps the doc accurate ("process
/// start" rather than "first health check"). Calling `build_id()` later
/// returns the same value because `OnceLock::get_or_init` is idempotent.
pub fn build_id() -> u128 {
    *BUILD_ID.get_or_init(now_millis)
}

/// Eagerly initialize [`build_id`] so the returned timestamp truly
/// represents process-start rather than first-call. Idempotent.
pub fn init_build_id() {
    let _ = BUILD_ID.get_or_init(now_millis);
}

static BUILD_ID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Absolute path of the workspace root the server was launched from —
/// captured once at process start. Surfaced via `/api/_health` so
/// `scripts/dev-server-up.sh` can tell *its own workspace's* server apart
/// from a sibling `jj` workspace's server that happens to be bound to the
/// port it's probing. Without this, port-walking would silently reuse a
/// sibling workspace's server (different code, different DB) and the
/// agent would validate against the wrong build.
///
/// `main.rs` calls [`init_repo_root`] eagerly during boot so the value is
/// set before any request can read it. `OnceLock::get_or_init` is
/// idempotent, so calling [`repo_root`] later returns the same value.
pub fn repo_root() -> &'static str {
    REPO_ROOT.get_or_init(current_dir_string)
}

/// Eagerly initialize [`repo_root`] from the process's current working
/// directory. Idempotent.
pub fn init_repo_root() {
    let _ = REPO_ROOT.get_or_init(current_dir_string);
}

static REPO_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn current_dir_string() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_default()
}

/// Running server's release version, captured once at boot.
///
/// `main.rs` calls [`init_app_version`] eagerly during boot, mirroring
/// [`init_build_id`]/[`init_repo_root`]. `OnceLock::get_or_init` is
/// idempotent, so calling [`app_version`] later returns the same value.
pub fn app_version() -> &'static str {
    APP_VERSION.get_or_init(read_app_version)
}

/// Eagerly initialize [`app_version`]. Idempotent.
pub fn init_app_version() {
    let _ = APP_VERSION.get_or_init(read_app_version);
}

static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Reads `OMNIBUS_VERSION` (baked into the Docker image at build time by
/// `docker.yml`) and normalizes it to a single leading `v` — the Docker
/// build-arg carries the full, already-`v`-prefixed release tag, but a
/// hand-set deployment env might not, and a doubled `vv1.2.3` shouldn't
/// happen either.
///
/// Falls back to the literal `"dev"` when the var is unset, empty/whitespace
/// (a build-arg supplied without a value sets the env to `""` rather than
/// leaving it unset), or literally just `"v"` after trimming — local `cargo
/// run`/`dx serve` builds have no release tag to report, and `"dev"` reads
/// unambiguously as "no release tag", never as a real version.
pub(super) fn read_app_version() -> String {
    let trimmed = std::env::var("OMNIBUS_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match trimmed {
        Some(v) => {
            let bare = v.trim_start_matches('v');
            if bare.is_empty() {
                "dev".to_string()
            } else {
                format!("v{bare}")
            }
        }
        None => "dev".to_string(),
    }
}
