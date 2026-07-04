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

static MISSING_WARNED: OnceLock<()> = OnceLock::new();

/// Log a single WARN if kepubify is not runnable. Idempotent — safe to call
/// at startup and again from the download fallback path without spamming.
pub fn warn_if_unavailable() {
    if !kepubify_available() {
        MISSING_WARNED.get_or_init(|| {
            tracing::warn!(
                target: "omnibus::kepub",
                "kepubify not found (set OMNIBUS_KEPUBIFY_PATH or install it); \
                 Kobo downloads fall back to plain EPUB with slower page turns"
            );
        });
    }
}
