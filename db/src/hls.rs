//! HLS transcode cache for audiobooks.
//!
//! One ffmpeg invocation per `(book_id, profile)` writes all segments
//! atomically to `$OMNIBUS_DATA_DIR/hls/<book_id>/<profile>/`. The only
//! profile today is `"audio64"` (AAC-LC 64 kbps mono, 10 s segments).
//! Segments are served directly by the axum handler via `tower-http`
//! `ServeFile`; the manifest is built per-request from the stored
//! `book_file_parts.duration_seconds` values so it is always accurate even
//! before a transcode completes.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use sqlx::SqlitePool;
use tokio::io::AsyncBufReadExt;

/// Errors returned by the HLS DB queries. `transcode_book` uses
/// `anyhow::Result` because ffmpeg + filesystem + timeouts dominate its
/// failure space and callers just propagate.
#[derive(Debug, thiserror::Error)]
pub enum HlsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The only HLS audio profile shipped today. Future: add `"audio128"` for
/// music audiobooks that warrant higher fidelity.
pub const AUDIO64: &str = "audio64";

/// Heartbeat cadence written to the `.progress` sentinel while ffmpeg is
/// alive. Long enough to keep filesystem traffic negligible; short enough
/// that [`STALE_PROGRESS_THRESHOLD`] can be three heartbeats wide without
/// the orphan detector mistaking a slow disk for a dead transcode.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(5);

/// Mtime age past which an in-flight `.progress` sentinel is treated as
/// orphaned (server crashed / hot-reloaded mid-transcode, leaving the
/// `0.x` value pinned forever without a live ffmpeg behind it).
/// Three heartbeats wide so transient I/O stalls don't trip it.
const STALE_PROGRESS_THRESHOLD: Duration = Duration::from_secs(30);

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
/// endpoint's re-kick gate fires naturally) and [`transcode_book`] (so a
/// worker picking up the task can wipe the orphan sentinel before
/// retrying).
fn is_progress_stale_at(path: &Path) -> bool {
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

/// One part of a multi-file audiobook as read from `book_file_parts`.
/// Only the fields needed by the HLS manifest builder and transcode runner.
#[derive(Debug, Clone)]
pub struct HlsPart {
    /// Playlist ordering (from `book_file_parts.ordinal`).
    pub ordinal: i64,
    /// Library-relative path (e.g. `"Author/Book/01.mp3"`).
    pub filename: String,
    /// Duration in seconds from `book_file_parts.duration_seconds`.
    pub duration_seconds: f64,
}

/// Resolved identifiers for an audiobook uuid lookup.
pub struct ResolvedAudiobook {
    /// `books.id`
    pub book_id: i64,
    /// `book_files.id`
    pub book_file_id: i64,
    /// `scan_roots.path` — the library root on disk.
    pub library_path: String,
}

/// Resolve a book `uuid` to the ids and library path needed by the HLS
/// handlers. When `file_id` is `Some`, resolve that specific `book_files`
/// row (verifying it belongs to the given uuid and is an audio format).
/// When `None`, returns the first audio file by ordinal. Returns `None`
/// when the uuid is unknown or no matching audio file exists.
pub async fn resolve_audiobook(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<ResolvedAudiobook>, HlsError> {
    resolve_audiobook_file(pool, uuid, None).await
}

/// Inner resolver that optionally targets a specific `book_files.id`.
pub async fn resolve_audiobook_file(
    pool: &SqlitePool,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<Option<ResolvedAudiobook>, HlsError> {
    let row = if let Some(fid) = file_id {
        sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT b.id, bf.id, COALESCE(bf.library_path, l.path) \
             FROM books b \
             JOIN book_files bf ON bf.book_id = b.id \
             JOIN scan_roots l ON l.id = b.library_id \
             WHERE b.uuid = ? \
               AND bf.id = ? \
               AND bf.format IN ('M4B', 'M4A', 'MP3')",
        )
        .bind(uuid)
        .bind(fid)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT b.id, bf.id, COALESCE(bf.library_path, l.path) \
             FROM books b \
             JOIN book_files bf ON bf.book_id = b.id \
             JOIN scan_roots l ON l.id = b.library_id \
             WHERE b.uuid = ? \
               AND bf.format IN ('M4B', 'M4A', 'MP3') \
             ORDER BY bf.ordinal \
             LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(pool)
        .await?
    };

    Ok(
        row.map(|(book_id, book_file_id, library_path)| ResolvedAudiobook {
            book_id,
            book_file_id,
            library_path,
        }),
    )
}

/// Fetch ordered `book_file_parts` for `book_file_id`.
pub async fn get_parts(pool: &SqlitePool, book_file_id: i64) -> Result<Vec<HlsPart>, HlsError> {
    let rows = sqlx::query_as::<_, (i64, String, f64)>(
        "SELECT ordinal, filename, duration_seconds \
         FROM book_file_parts \
         WHERE book_file_id = ? \
         ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(ordinal, filename, duration_seconds)| HlsPart {
            ordinal,
            filename,
            duration_seconds,
        })
        .collect())
}

/// Fetch chapter markers for a `book_file_id`. Always returns at least one
/// row because the sync layer writes synthetic chapters when none are
/// extracted from the container.
pub async fn get_chapters(
    pool: &SqlitePool,
    book_file_id: i64,
) -> Result<Vec<omnibus_shared::ChapterInfo>, HlsError> {
    let rows = sqlx::query_as::<_, (i64, String, f64, f64)>(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters \
         WHERE book_file_id = ? \
         ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(ordinal, title, start_seconds, duration_seconds)| omnibus_shared::ChapterInfo {
                ordinal,
                title,
                start_seconds,
                duration_seconds,
            },
        )
        .collect())
}

/// Build an HLS VOD manifest from the stored part durations.
///
/// Each segment is 10 seconds; the last segment gets the remainder.
/// When all parts have `duration_seconds = 0` (not yet lofty-probed) a
/// minimal single-segment stub is returned so the frontend can still load the
/// resource before the first real transcode finishes.
pub fn build_manifest(parts: &[HlsPart]) -> String {
    const TARGET: f64 = 10.0;
    let total_secs: f64 = parts.iter().map(|p| p.duration_seconds).sum();

    if total_secs <= 0.0 {
        // Minimal stub so hls.js/Safari can discover the URL is valid before
        // the duration probe / transcode finishes.
        return concat!(
            "#EXTM3U\n",
            "#EXT-X-VERSION:3\n",
            "#EXT-X-TARGETDURATION:10\n",
            "#EXT-X-PLAYLIST-TYPE:VOD\n",
            "#EXT-X-MEDIA-SEQUENCE:0\n",
            "#EXTINF:0.001,\n",
            "seg-0000.ts\n",
            "#EXT-X-ENDLIST\n"
        )
        .to_string();
    }

    let num_segments = (total_secs / TARGET).ceil() as usize;
    let mut m3u8 = String::with_capacity(num_segments * 40);
    m3u8.push_str("#EXTM3U\n");
    m3u8.push_str("#EXT-X-VERSION:3\n");
    m3u8.push_str("#EXT-X-TARGETDURATION:10\n");
    m3u8.push_str("#EXT-X-PLAYLIST-TYPE:VOD\n");
    m3u8.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");

    for i in 0..num_segments {
        let dur = if i == num_segments - 1 {
            total_secs - (i as f64) * TARGET
        } else {
            TARGET
        };
        m3u8.push_str(&format!("#EXTINF:{:.3},\nseg-{:04}.ts\n", dur, i));
    }
    m3u8.push_str("#EXT-X-ENDLIST\n");
    m3u8
}

/// Parse one ffmpeg `-progress pipe:1` line and return the encode timeline
/// position in microseconds when the line is the `out_time_us=<int>` (or
/// `out_time_ms=<int>`, ffmpeg confusingly uses microseconds for both)
/// progress key. Returns `None` for every other key — `frame=`, `bitrate=`,
/// `progress=continue`, etc. Public for unit testing.
pub fn parse_ffmpeg_progress_us(line: &str) -> Option<u64> {
    let line = line.trim();
    let (key, value) = line.split_once('=')?;
    // ffmpeg has historically emitted both `out_time_us=` (named correctly
    // since ~5.0) and `out_time_ms=` (a long-standing typo that actually
    // carries microseconds). Accept both so the parser keeps working across
    // versions.
    if key != "out_time_us" && key != "out_time_ms" {
        return None;
    }
    let value = value.trim();
    // `N/A` shows up during the initial warmup before ffmpeg has produced
    // any output frames. Drop it cleanly rather than treating it as 0.
    if value == "N/A" {
        return None;
    }
    value.parse::<u64>().ok()
}

/// Compute the heartbeat fraction to write to `.progress` from an
/// `out_time_us` value and the total duration in seconds. Clamped to
/// `[0.01, 0.95]` so the orphan detector always sees motion and the
/// success sentinel `1.0` is reserved for [`finalize_transcode`].
/// Public for unit testing.
pub fn ffmpeg_progress_fraction(out_time_us: u64, total_secs: f64) -> f32 {
    if total_secs <= 0.0 {
        return 0.05;
    }
    let elapsed_secs = (out_time_us as f64) / 1_000_000.0;
    (elapsed_secs / total_secs).clamp(0.01, 0.95) as f32
}

/// Transcode all parts for `(book_id, profile)` to HLS segments via ffmpeg.
///
/// Writes a `.progress` sentinel that heartbeats every
/// [`HEARTBEAT_PERIOD`] (~5 s) while ffmpeg is alive, and `1.0` on
/// success. If a previous run left a `.progress` sentinel pinned to a
/// non-terminal value (server restart mid-transcode), the orphan is
/// detected via [`is_progress_stale_at`] and wiped before retrying so the
/// status endpoint's `progress < 0.05` re-kick gate can fire on the next
/// poll.
///
/// On failure the sentinel and all partial segments are cleaned up so a
/// retry produces a clean slate, and a sibling `.failed` marker is left
/// behind so subsequent polls don't burn CPU on a corrupt source.
///
/// Configurable via:
/// - `OMNIBUS_FFMPEG_PATH` — path or name of the ffmpeg binary (default `ffmpeg`)
/// - `OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS` — hard timeout in seconds (default 1800)
pub async fn transcode_book(
    pool: &SqlitePool,
    book_id: i64,
    book_file_id: i64,
    library_path: &str,
    profile: &str,
) -> anyhow::Result<()> {
    // Two early-returns before any ffmpeg work so N concurrent listeners
    // of the same book don't re-transcode after the first finishes, and a
    // permanently-broken book doesn't burn CPU on every status poll.
    if is_ready(book_id, profile) {
        return Ok(());
    }
    if has_failed(book_id, profile) {
        anyhow::bail!(
            "transcode previously failed; remove {} to retry",
            failed_path(book_id, profile).display()
        );
    }

    let parts = get_parts(pool, book_file_id).await?;
    if parts.is_empty() {
        anyhow::bail!("book_id={book_id}: no book_file_parts rows found");
    }

    let outdir = segment_dir(book_id, profile);
    std::fs::create_dir_all(&outdir)?;

    // Self-healing: if a previous run left an orphan `.progress` (server
    // restart mid-transcode), wipe it before spawning a fresh ffmpeg so
    // the next status poll sees the kick land as a brand-new transcode.
    // Safe to do unconditionally — we hold the worker's per-resource
    // `hls:{book_id}:{profile}` mutex, so no other transcode for this book
    // can be live behind the sentinel.
    let progress_file = progress_path(book_id, profile);
    if progress_file.exists() && is_progress_stale_at(&progress_file) {
        if let Err(e) = std::fs::remove_file(&progress_file) {
            tracing::warn!(
                book_id = book_id,
                profile = profile,
                error = %e,
                "failed to remove orphan .progress sentinel"
            );
        }
    }

    let concat_path = write_concat_input(&outdir, library_path, &parts)?;

    // Bootstrap heartbeat: write a tiny non-zero value so a racing
    // status-poll between `post(...)` and the first parsed `out_time_us`
    // line still sees motion. The async progress task will overwrite this
    // on its first tick.
    let _ = std::fs::write(&progress_file, "0.01");

    let total_secs: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    let outcome = run_ffmpeg_with_progress(book_id, profile, &concat_path, &outdir, total_secs)
        .await
        .map_err(anyhow::Error::msg);
    finalize_transcode(book_id, profile, outcome)
}

/// Possible outcomes of one ffmpeg invocation, as observed by
/// [`run_ffmpeg_with_progress`].
enum FfmpegOutcome {
    Success,
    /// ffmpeg exited non-zero. Carries the captured stderr for the log.
    NonZero {
        status: std::process::ExitStatus,
        stderr: String,
    },
    /// The watchdog elapsed before ffmpeg finished.
    Timeout {
        timeout_secs: u64,
    },
}

/// Write the `concat.txt` input that ffmpeg's concat demuxer reads. Each
/// part becomes one `file '<abs-path>'` line with single-quote escaping.
fn write_concat_input(
    outdir: &Path,
    library_path: &str,
    parts: &[HlsPart],
) -> std::io::Result<PathBuf> {
    let concat_path = outdir.join("concat.txt");
    let mut content = String::new();
    for p in parts {
        let abs = Path::new(library_path).join(&p.filename);
        // ffmpeg concat demuxer requires single-quotes escaped as '\'' and
        // the rest of the path double-escaped for the `file` directive.
        let abs_str = abs.to_string_lossy().replace('\'', "'\\''");
        content.push_str(&format!("file '{}'\n", abs_str));
    }
    std::fs::write(&concat_path, &content)?;
    Ok(concat_path)
}

/// Spawn ffmpeg with `-progress pipe:1 -nostats`, drain its stdout into
/// a heartbeat task that writes [`progress_path`] every
/// [`HEARTBEAT_PERIOD`], and wait for the process under a wall-clock
/// timeout (`OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS`, default 1800 s).
async fn run_ffmpeg_with_progress(
    book_id: i64,
    profile: &str,
    concat_path: &Path,
    outdir: &Path,
    total_secs: f64,
) -> Result<FfmpegOutcome, String> {
    let timeout_secs: u64 = std::env::var("OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    let mut child = spawn_ffmpeg(concat_path, outdir)?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "ffmpeg stdout pipe missing".to_string())?;

    let progress_file = progress_path(book_id, profile);
    let progress_task = tokio::spawn(stream_progress(stdout, progress_file, total_secs));

    wait_for_ffmpeg(child, progress_task, timeout_secs).await
}

/// Spawn the ffmpeg child process for HLS transcoding with piped stdout/stderr.
fn spawn_ffmpeg(concat_path: &Path, outdir: &Path) -> Result<tokio::process::Child, String> {
    let ffmpeg = std::env::var("OMNIBUS_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let seg_pattern = outdir.join("seg-%04d.ts");
    let manifest_out = outdir.join("index.m3u8");

    tokio::process::Command::new(&ffmpeg)
        .args([
            "-f",
            "concat",
            "-safe",
            "0",
            "-i",
            &concat_path.to_string_lossy(),
            "-c:a",
            "aac",
            "-b:a",
            "64k",
            "-ac",
            "1",
            "-ar",
            "44100",
            "-hls_time",
            "10",
            "-hls_playlist_type",
            "vod",
            "-hls_segment_type",
            "mpegts",
            "-hls_segment_filename",
            &seg_pattern.to_string_lossy(),
            "-start_number",
            "0",
            "-progress",
            "pipe:1",
            "-nostats",
            "-y",
            &manifest_out.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("ffmpeg spawn failed: {e}"))
}

/// Await ffmpeg exit under `timeout_secs`, then map the result to
/// [`FfmpegOutcome`]. Aborts the progress-heartbeat task before returning
/// on the timeout path so we don't depend on the kill→EOF chain to unblock
/// the `next_line` await inside the reader.
async fn wait_for_ffmpeg(
    child: tokio::process::Child,
    progress_task: tokio::task::JoinHandle<()>,
    timeout_secs: u64,
) -> Result<FfmpegOutcome, String> {
    let wait_result = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await;

    match wait_result {
        Ok(Ok(output)) if output.status.success() => {
            let _ = progress_task.await;
            Ok(FfmpegOutcome::Success)
        }
        Ok(Ok(output)) => {
            let _ = progress_task.await;
            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            Ok(FfmpegOutcome::NonZero {
                status: output.status,
                stderr,
            })
        }
        Ok(Err(io_err)) => {
            let _ = progress_task.await;
            Err(format!("ffmpeg wait failed: {io_err}"))
        }
        Err(_elapsed) => {
            // `kill_on_drop(true)` reaps the child when the wait future is
            // dropped on the timeout return path; abort the progress reader
            // explicitly so we don't depend on the kill → EOF chain to
            // unblock its `next_line` await.
            progress_task.abort();
            let _ = progress_task.await;
            Ok(FfmpegOutcome::Timeout { timeout_secs })
        }
    }
}

/// Read ffmpeg's `-progress pipe:1` stream line-by-line and write the
/// current encode fraction to [`progress_path`] no more than once per
/// [`HEARTBEAT_PERIOD`]. The cadence is throttled so a slow source
/// doesn't burn fs syscalls on every `out_time_us=` tick.
async fn stream_progress(
    stdout: tokio::process::ChildStdout,
    progress_file: PathBuf,
    total_secs: f64,
) {
    let mut reader = tokio::io::BufReader::new(stdout).lines();
    // Allow the first parsed tick to write immediately so a racing
    // status-poll sees real motion within the first ~second instead of
    // waiting an entire heartbeat period for the bootstrap to lift.
    let mut last_write: Option<std::time::Instant> = None;
    loop {
        match reader.next_line().await {
            Ok(Some(line)) => {
                let Some(us) = parse_ffmpeg_progress_us(&line) else {
                    continue;
                };
                let due = match last_write {
                    None => true,
                    Some(t) => t.elapsed() >= HEARTBEAT_PERIOD,
                };
                if !due {
                    continue;
                }
                let pct = ffmpeg_progress_fraction(us, total_secs);
                let _ = tokio::fs::write(&progress_file, format!("{pct}")).await;
                last_write = Some(std::time::Instant::now());
            }
            Ok(None) => break, // EOF: ffmpeg closed stdout, we're done.
            Err(_) => break,
        }
    }
}

/// Apply the success / failure / timeout side-effects of one ffmpeg run.
/// Split out of [`transcode_book`] so the spawn + heartbeat + wait stages
/// can stay under the function-length cap.
fn finalize_transcode(
    book_id: i64,
    profile: &str,
    outcome: Result<FfmpegOutcome, anyhow::Error>,
) -> anyhow::Result<()> {
    match outcome {
        Ok(FfmpegOutcome::Success) => {
            std::fs::write(progress_path(book_id, profile), "1.0")?;
            let cap = cap_bytes();
            if let Err(e) = evict_if_over_cap(cap) {
                tracing::warn!(error = %e, "HLS eviction failed after successful transcode");
            }
            Ok(())
        }
        Ok(FfmpegOutcome::NonZero { status, stderr }) => {
            tracing::error!(
                book_id = book_id,
                profile = profile,
                status = ?status,
                stderr = %stderr,
                "ffmpeg transcode failed"
            );
            cleanup_segment_dir(book_id, profile);
            anyhow::bail!("ffmpeg exited with status {status}")
        }
        Ok(FfmpegOutcome::Timeout { timeout_secs }) => {
            tracing::error!(
                book_id = book_id,
                profile = profile,
                timeout_secs = timeout_secs,
                "ffmpeg transcode timed out"
            );
            cleanup_segment_dir(book_id, profile);
            anyhow::bail!("ffmpeg transcode timed out after {timeout_secs}s")
        }
        Err(e) => {
            tracing::error!(book_id = book_id, error = %e, "ffmpeg spawn/wait error");
            cleanup_segment_dir(book_id, profile);
            Err(e)
        }
    }
}

/// Remove a `(book_id, profile)` segment directory and its `.progress`
/// sentinel on transcode failure, then write a sibling `.failed` marker
/// so the status / segment handlers stop kicking fresh transcodes for
/// what is clearly a broken book (corrupt source, missing ffmpeg, …).
///
/// The `.failed` marker lives at the `<book_id>/` level (not inside the
/// just-removed segment dir) so a retry is only possible after either
/// (a) operator intervention removing the marker, or (b) a cache
/// eviction sweep that wipes the entire `<book_id>/` directory.
fn cleanup_segment_dir(book_id: i64, profile: &str) {
    let dir = segment_dir(book_id, profile);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        tracing::warn!(path = ?dir, error = %e, "failed to clean up segment dir after error");
    }
    let book_dir = hls_dir().join(book_id.to_string());
    if let Err(e) = std::fs::create_dir_all(&book_dir) {
        tracing::warn!(path = ?book_dir, error = %e, "failed to create book dir for .failed marker");
        return;
    }
    let marker = failed_path(book_id, profile);
    if let Err(e) = std::fs::write(&marker, "") {
        tracing::warn!(path = ?marker, error = %e, "failed to write .failed marker");
    }
}

/// Evict oldest book-level cache directories until total usage ≤ `cap_bytes`.
///
/// Directories are sorted by their own modification time (oldest first). Each
/// is removed in order until the aggregate drops below the cap. Removal errors
/// are logged as WARN and the loop continues — a single locked file should not
/// prevent other dirs from being evicted.
pub fn evict_if_over_cap(cap_bytes: u64) -> anyhow::Result<()> {
    let dir = hls_dir();
    if !dir.exists() {
        return Ok(());
    }

    // Collect `(mtime, book_dir_path)` for every immediate child of `hls_dir`.
    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(&dir)?.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let mtime = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .unwrap_or(std::time::UNIX_EPOCH);
        entries.push((mtime, path));
    }

    let total_bytes: u64 = entries.iter().map(|(_, p)| dir_size(p)).sum();
    if total_bytes <= cap_bytes {
        return Ok(());
    }

    // Oldest mtime first.
    entries.sort_by_key(|(t, _)| *t);

    let mut remaining = total_bytes;
    for (_, path) in &entries {
        if remaining <= cap_bytes {
            break;
        }
        let sz = dir_size(path);
        match std::fs::remove_dir_all(path) {
            Ok(()) => {
                remaining = remaining.saturating_sub(sz);
                tracing::info!(path = ?path, freed_bytes = sz, "HLS cache eviction: removed");
            }
            Err(e) => {
                tracing::warn!(path = ?path, error = %e, "HLS cache eviction failed");
            }
        }
    }
    Ok(())
}

/// Recursively sum the sizes of all regular files under `path`. Errors
/// (permission denied, symlink loops) are silently skipped — this is a
/// best-effort estimate used only for eviction decisions.
fn dir_size(path: &std::path::Path) -> u64 {
    let mut total = 0u64;
    let Ok(rd) = std::fs::read_dir(path) else {
        return 0;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            total += dir_size(&p);
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[cfg(test)]
mod tests;
