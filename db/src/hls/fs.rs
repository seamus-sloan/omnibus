//! Filesystem + config helpers for the HLS cache: directory layout,
//! sentinel paths, env-driven cap, and the progress / failed-marker
//! read helpers used by both the status handler and the transcode runner.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Mtime age past which an in-flight `.progress` sentinel is treated as
/// orphaned (server crashed / hot-reloaded mid-transcode, leaving the
/// `0.x` value pinned forever without a live ffmpeg behind it).
/// Six `HEARTBEAT_PERIOD` ticks (5 s × 6 = 30 s) so transient I/O stalls
/// don't trip it.
pub(super) const STALE_PROGRESS_THRESHOLD: Duration = Duration::from_secs(30);

/// Root directory for the HLS segment cache.
///
/// Reads `$OMNIBUS_DATA_DIR` (default `./data`).
pub fn hls_dir() -> PathBuf {
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("hls")
}

/// Cache directory for one `(book_id, profile)` pair.
pub fn segment_dir(book_id: i64, profile: &str) -> PathBuf {
    hls_dir().join(book_id.to_string()).join(profile)
}

/// Maximum total bytes across the whole HLS cache before eviction kicks in.
///
/// Reads `$OMNIBUS_HLS_CAP_BYTES` (default 5 GiB).
pub fn cap_bytes() -> u64 {
    std::env::var("OMNIBUS_HLS_CAP_BYTES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5 * 1024 * 1024 * 1024)
}

/// Path to the `.progress` sentinel file for `(book_id, profile)`. The file
/// contains a single `f32` as a decimal string. While ffmpeg is running
/// the value advances from `0.0+` toward `0.95` (live heartbeat from the
/// `-progress pipe:1` parser); on success the sentinel is overwritten with
/// `1.0`.
pub fn progress_path(book_id: i64, profile: &str) -> PathBuf {
    segment_dir(book_id, profile).join(".progress")
}

/// Read transcode progress `[0.0, 1.0]` from the sentinel file. Returns
/// `0.0` if the file is absent, unreadable, or stale (mtime older than
/// [`STALE_PROGRESS_THRESHOLD`] and value < 1.0). Treating stale sentinels
/// as zero lets the status handler's `progress < 0.05` re-kick gate fire
/// naturally after a crashed/restarted transcode — without that, an orphan
/// `.progress=0.1` left over from a previous run would pin the book in
/// "preparing" forever.
pub fn read_progress(book_id: i64, profile: &str) -> f32 {
    let path = progress_path(book_id, profile);
    let raw: f32 = match std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse().ok())
    {
        Some(v) => v,
        None => return 0.0,
    };
    // Treat a complete transcode as live no matter the mtime — eviction is
    // mtime-driven (FIFO) and we don't want a long-idle book to look like
    // an orphan to a polling client.
    if raw >= 1.0 {
        return raw;
    }
    if is_progress_stale_at(&path) {
        return 0.0;
    }
    raw
}

/// `true` when the transcode for `(book_id, profile)` is complete
/// (progress sentinel ≈ 1.0).
pub fn is_ready(book_id: i64, profile: &str) -> bool {
    (read_progress(book_id, profile) - 1.0_f32).abs() < 0.01
}

/// Path to the terminal-failure sentinel for `(book_id, profile)`. Lives
/// at the `<book_id>/` level (one level above the segment dir) so the
/// cleanup pass that wipes the segment dir on failure can still leave the
/// flag behind. Removed when the parent `<book_id>/` dir is evicted, which
/// is the explicit "give it a fresh chance" signal.
pub fn failed_path(book_id: i64, profile: &str) -> PathBuf {
    hls_dir()
        .join(book_id.to_string())
        .join(format!("{profile}.failed"))
}

/// `true` when a previous transcode for `(book_id, profile)` terminally
/// failed (e.g. ffmpeg exited non-zero or timed out). Callers should NOT
/// kick a fresh `Task::HlsTranscode` while this is true — the status
/// endpoint's 1 s poll would otherwise become an unbounded retry loop on
/// a corrupt source.
pub fn has_failed(book_id: i64, profile: &str) -> bool {
    failed_path(book_id, profile).exists()
}

/// Path of the ffmpeg-produced HLS manifest for `(book_id, profile)`.
/// Available once `is_ready` is true.
pub fn ffmpeg_manifest_path(book_id: i64, profile: &str) -> PathBuf {
    segment_dir(book_id, profile).join("index.m3u8")
}

/// Read the ffmpeg-produced HLS manifest from disk. `None` when missing
/// or unreadable (eviction race, fs error) — the caller falls back to
/// the DB-built stub in that case.
pub fn read_ffmpeg_manifest(book_id: i64, profile: &str) -> Option<String> {
    std::fs::read_to_string(ffmpeg_manifest_path(book_id, profile)).ok()
}

/// `true` when `path` exists, its mtime is older than
/// [`STALE_PROGRESS_THRESHOLD`]. Used by [`read_progress`] (so the status
/// endpoint's re-kick gate fires naturally) and `transcode_book` (so a
/// worker picking up the task can wipe the orphan sentinel before
/// retrying).
pub(super) fn is_progress_stale_at(path: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    let Ok(age) = SystemTime::now().duration_since(mtime) else {
        // mtime is in the future (clock skew). Don't call that stale.
        return false;
    };
    age >= STALE_PROGRESS_THRESHOLD
}
