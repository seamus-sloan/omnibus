//! kepubify binary detection + the one-time "not installed" warning.

use std::sync::OnceLock;

/// The kepubify binary to invoke: `$OMNIBUS_KEPUBIFY_PATH`, else `kepubify`
/// resolved on `$PATH` (mirrors `OMNIBUS_FFMPEG_PATH`).
pub(super) fn kepubify_bin() -> String {
    std::env::var("OMNIBUS_KEPUBIFY_PATH").unwrap_or_else(|_| "kepubify".into())
}

/// `true` when the kepubify binary is present and runnable (`--version`
/// exits 0). A blocking probe — call at startup or off the hot path.
pub fn kepubify_available() -> bool {
    std::process::Command::new(kepubify_bin())
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

static PROBED: OnceLock<()> = OnceLock::new();

/// Log a single WARN if kepubify is not runnable. Runs the `kepubify --version`
/// probe **at most once per process**, so a repeated caller (e.g. a download
/// fallback path) never re-spawns the subprocess.
pub fn warn_if_unavailable() {
    // `set` succeeds exactly once; every later call short-circuits before the
    // blocking availability probe.
    if PROBED.set(()).is_err() {
        return;
    }
    if !kepubify_available() {
        tracing::warn!(
            target: "omnibus::kepub",
            "kepubify not found (set OMNIBUS_KEPUBIFY_PATH or install it); \
             Kobo downloads fall back to plain EPUB with slower page turns"
        );
    }
}
