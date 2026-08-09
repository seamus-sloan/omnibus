//! `ebook-convert` (Calibre) binary detection + the one-time "not installed"
//! warning. Mirrors `crate::kepub::detect`: Calibre stays optional, so an
//! unresolved binary disables conversion rather than failing the boot.

use std::sync::OnceLock;

use omnibus_shared::DEFAULT_EBOOK_CONVERT_BIN;

/// The `ebook-convert` binary to invoke when no settings override is saved:
/// `$OMNIBUS_EBOOK_CONVERT_PATH`, else `ebook-convert` resolved on `$PATH`
/// (mirrors `OMNIBUS_KEPUBIFY_PATH` / `OMNIBUS_FFMPEG_PATH`).
pub fn ebook_convert_bin() -> String {
    std::env::var("OMNIBUS_EBOOK_CONVERT_PATH")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_EBOOK_CONVERT_BIN.to_string())
}

/// `true` when `bin` is present and runnable (`--version` exits 0). A blocking
/// probe — call at startup or off the hot path.
pub fn is_runnable(bin: &str) -> bool {
    std::process::Command::new(bin)
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// `true` when the env/`$PATH`-resolved `ebook-convert` is runnable.
pub fn ebook_convert_available() -> bool {
    is_runnable(&ebook_convert_bin())
}

static PROBED: OnceLock<()> = OnceLock::new();

/// Log a single WARN if `ebook-convert` is not runnable. Runs the
/// `ebook-convert --version` probe **at most once per process**, so a repeated
/// caller never re-spawns the subprocess.
pub fn warn_if_unavailable() {
    // `set` succeeds exactly once; every later call short-circuits before the
    // blocking availability probe.
    if PROBED.set(()).is_err() {
        return;
    }
    if !ebook_convert_available() {
        tracing::warn!(
            target: "omnibus::convert",
            "ebook-convert not found (install Calibre or set OMNIBUS_EBOOK_CONVERT_PATH); \
             format conversion is disabled"
        );
    }
}
