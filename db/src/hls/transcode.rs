//! ffmpeg transcode engine for the HLS cache: orchestrates a single
//! `(book_id, profile)` run end-to-end — concat-input prep, child spawn
//! with live `-progress pipe:1` heartbeat, watchdog timeout, success /
//! failure / timeout finalize, and FIFO-by-mtime cache eviction.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use sqlx::SqlitePool;
use tokio::io::AsyncBufReadExt;

use super::fs::is_progress_stale_at;
use super::{
    cap_bytes, failed_path, ffmpeg_progress_fraction, get_parts, has_failed, hls_dir, is_ready,
    parse_ffmpeg_progress_us, progress_path, segment_dir, HlsPart,
};

/// Heartbeat cadence written to the `.progress` sentinel while ffmpeg is
/// alive. Long enough to keep filesystem traffic negligible; short enough
/// that the stale-progress threshold can be six heartbeats wide without
/// the orphan detector mistaking a slow disk for a dead transcode.
const HEARTBEAT_PERIOD: Duration = Duration::from_secs(5);

/// Transcode all parts for `(book_id, profile)` to HLS segments via ffmpeg.
///
/// Writes a `.progress` sentinel that heartbeats every
/// [`HEARTBEAT_PERIOD`] (~5 s) while ffmpeg is alive, and `1.0` on
/// success. If a previous run left a `.progress` sentinel pinned to a
/// non-terminal value (server restart mid-transcode), the orphan is
/// detected via `is_progress_stale_at` and wiped before retrying so the
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
    tokio::fs::create_dir_all(&outdir).await?;

    // Self-healing: if a previous run left an orphan `.progress` (server
    // restart mid-transcode), wipe it before spawning a fresh ffmpeg so
    // the next status poll sees the kick land as a brand-new transcode.
    // Safe to do unconditionally — we hold the worker's per-resource
    // `hls:{book_id}:{profile}` mutex, so no other transcode for this book
    // can be live behind the sentinel.
    let progress_file = progress_path(book_id, profile);
    if progress_file.exists() && is_progress_stale_at(&progress_file) {
        if let Err(e) = tokio::fs::remove_file(&progress_file).await {
            tracing::warn!(
                book_id = book_id,
                profile = profile,
                error = %e,
                "failed to remove orphan .progress sentinel"
            );
        }
    }

    let concat_path = write_concat_input(&outdir, library_path, &parts).await?;

    // Bootstrap heartbeat: write a tiny non-zero value so a racing
    // status-poll between `post(...)` and the first parsed `out_time_us`
    // line still sees motion. The async progress task will overwrite this
    // on its first tick.
    let _ = tokio::fs::write(&progress_file, "0.01").await;

    let total_secs: f64 = parts.iter().map(|p| p.duration_seconds).sum();
    let outcome =
        run_ffmpeg_with_progress(book_id, profile, &concat_path, &outdir, total_secs).await;
    finalize_transcode(book_id, profile, outcome).await
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
async fn write_concat_input(
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
    tokio::fs::write(&concat_path, &content).await?;
    Ok(concat_path)
}

/// Spawn ffmpeg with `-progress pipe:1 -nostats`, drain its stdout into
/// a heartbeat task that writes `progress_path` every [`HEARTBEAT_PERIOD`],
/// and wait for the process under a wall-clock timeout
/// (`OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS`, default 1800 s).
async fn run_ffmpeg_with_progress(
    book_id: i64,
    profile: &str,
    concat_path: &Path,
    outdir: &Path,
    total_secs: f64,
) -> anyhow::Result<FfmpegOutcome> {
    let timeout_secs: u64 = std::env::var("OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    let mut child = spawn_ffmpeg(concat_path, outdir)?;
    let stdout = child.stdout.take().context("ffmpeg stdout pipe missing")?;

    let progress_file = progress_path(book_id, profile);
    let progress_task = tokio::spawn(stream_progress(stdout, progress_file, total_secs));

    wait_for_ffmpeg(child, progress_task, timeout_secs).await
}

/// Spawn the ffmpeg child process for HLS transcoding with piped stdout/stderr.
fn spawn_ffmpeg(concat_path: &Path, outdir: &Path) -> anyhow::Result<tokio::process::Child> {
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
        .with_context(|| format!("ffmpeg spawn failed (binary: {ffmpeg})"))
}

/// Await ffmpeg exit under `timeout_secs`, then map the result to
/// [`FfmpegOutcome`]. Aborts the progress-heartbeat task before returning
/// on the timeout path so we don't depend on the kill→EOF chain to unblock
/// the `next_line` await inside the reader.
async fn wait_for_ffmpeg(
    child: tokio::process::Child,
    progress_task: tokio::task::JoinHandle<()>,
    timeout_secs: u64,
) -> anyhow::Result<FfmpegOutcome> {
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
            Err(anyhow::Error::new(io_err).context("ffmpeg wait failed"))
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
/// current encode fraction to `progress_path` no more than once per
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
            Err(e) => {
                tracing::warn!(error = %e, "ffmpeg stdout heartbeat read failed");
                break;
            }
        }
    }
}

/// Apply the success / failure / timeout side-effects of one ffmpeg run.
/// Split out of [`transcode_book`] so the spawn + heartbeat + wait stages
/// can stay under the function-length cap.
async fn finalize_transcode(
    book_id: i64,
    profile: &str,
    outcome: Result<FfmpegOutcome, anyhow::Error>,
) -> anyhow::Result<()> {
    match outcome {
        Ok(FfmpegOutcome::Success) => {
            tokio::fs::write(progress_path(book_id, profile), "1.0").await?;
            // Eviction walks every book dir under the HLS cache (recursive
            // read_dir + remove_dir_all). Push it onto the blocking pool so
            // a packed cache can't stall the worker thread for hundreds of
            // ms while ffmpeg's own progress reader keeps draining.
            let cap = cap_bytes();
            let evict_res = tokio::task::spawn_blocking(move || evict_if_over_cap(cap)).await;
            match evict_res {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    tracing::warn!(error = %e, "HLS eviction failed after successful transcode");
                }
                Err(join_err) => {
                    tracing::warn!(error = %join_err, "HLS eviction task panicked or was cancelled");
                }
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
            cleanup_segment_dir(book_id, profile).await;
            anyhow::bail!("ffmpeg exited with status {status}")
        }
        Ok(FfmpegOutcome::Timeout { timeout_secs }) => {
            tracing::error!(
                book_id = book_id,
                profile = profile,
                timeout_secs = timeout_secs,
                "ffmpeg transcode timed out"
            );
            cleanup_segment_dir(book_id, profile).await;
            anyhow::bail!("ffmpeg transcode timed out after {timeout_secs}s")
        }
        Err(e) => {
            tracing::error!(book_id = book_id, error = %e, "ffmpeg spawn/wait error");
            cleanup_segment_dir(book_id, profile).await;
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
async fn cleanup_segment_dir(book_id: i64, profile: &str) {
    let dir = segment_dir(book_id, profile);
    if let Err(e) = tokio::fs::remove_dir_all(&dir).await {
        tracing::warn!(path = ?dir, error = %e, "failed to clean up segment dir after error");
    }
    let book_dir = hls_dir().join(book_id.to_string());
    if let Err(e) = tokio::fs::create_dir_all(&book_dir).await {
        tracing::warn!(path = ?book_dir, error = %e, "failed to create book dir for .failed marker");
        return;
    }
    let marker = failed_path(book_id, profile);
    if let Err(e) = tokio::fs::write(&marker, "").await {
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

/// Total bytes currently held by the HLS transcode cache — every regular
/// file under `hls_dir()`, recursively. Read-only counterpart to
/// [`evict_if_over_cap`]'s usage computation, for the admin health page's
/// storage section (#952). Missing directory reads as empty rather than an
/// error: no transcode has run yet.
pub fn used_bytes() -> u64 {
    let dir = hls_dir();
    if !dir.exists() {
        return 0;
    }
    dir_size(&dir)
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
