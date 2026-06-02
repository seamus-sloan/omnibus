//! HLS transcode cache for audiobooks (F2.3).
//!
//! One ffmpeg invocation per `(book_id, profile)` writes all segments
//! atomically to `$OMNIBUS_DATA_DIR/hls/<book_id>/<profile>/`. The only
//! profile today is `"audio64"` (AAC-LC 64 kbps mono, 10 s segments).
//! Segments are served directly by the axum handler via `tower-http`
//! `ServeFile`; the manifest is built per-request from the stored
//! `book_file_parts.duration_seconds` values so it is always accurate even
//! before a transcode completes.

use std::path::PathBuf;

use sqlx::SqlitePool;

/// The only HLS audio profile shipped today. Future: add `"audio128"` for
/// music audiobooks that warrant higher fidelity.
pub const AUDIO64: &str = "audio64";

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
/// contains a single `f32` as a decimal string (`0.1` = started, `1.0` =
/// complete).
pub fn progress_path(book_id: i64, profile: &str) -> PathBuf {
    segment_dir(book_id, profile).join(".progress")
}

/// Read transcode progress `[0.0, 1.0]` from the sentinel file. Returns
/// `0.0` if the file is absent or unreadable.
pub fn read_progress(book_id: i64, profile: &str) -> f32 {
    std::fs::read_to_string(progress_path(book_id, profile))
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(0.0)
}

/// `true` when the transcode for `(book_id, profile)` is complete
/// (progress sentinel ≈ 1.0).
pub fn is_ready(book_id: i64, profile: &str) -> bool {
    (read_progress(book_id, profile) - 1.0_f32).abs() < 0.01
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
    /// `libraries.path` — the library root on disk.
    pub library_path: String,
}

/// Resolve a book `uuid` to the ids and library path needed by the HLS
/// handlers. Returns `None` when the uuid is unknown or the book has no
/// audiobook format (`M4B` / `M4A` / `MP3`) `book_files` row.
pub async fn resolve_audiobook(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<ResolvedAudiobook>, sqlx::Error> {
    let row = sqlx::query_as::<_, (i64, i64, String)>(
        "SELECT b.id, bf.id, l.path \
         FROM books b \
         JOIN book_files bf ON bf.book_id = b.id \
         JOIN libraries l ON l.id = b.library_id \
         WHERE b.uuid = ? \
           AND bf.format IN ('M4B', 'M4A', 'MP3') \
         LIMIT 1",
    )
    .bind(uuid)
    .fetch_optional(pool)
    .await?;

    Ok(
        row.map(|(book_id, book_file_id, library_path)| ResolvedAudiobook {
            book_id,
            book_file_id,
            library_path,
        }),
    )
}

/// Fetch ordered `book_file_parts` for `book_file_id`.
pub async fn get_parts(pool: &SqlitePool, book_file_id: i64) -> Result<Vec<HlsPart>, sqlx::Error> {
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

/// Transcode all parts for `(book_id, profile)` to HLS segments via ffmpeg.
///
/// Writes a `.progress` sentinel (`0.1` = started, `1.0` = done) so the
/// status endpoint can surface readiness without scanning the segment dir.
/// On failure the sentinel and all partial segments are cleaned up so a
/// retry produces a clean slate.
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
    let parts = get_parts(pool, book_file_id).await?;
    if parts.is_empty() {
        anyhow::bail!("book_id={book_id}: no book_file_parts rows found");
    }

    let outdir = segment_dir(book_id, profile);
    std::fs::create_dir_all(&outdir)?;

    // Write a concat input file so ffmpeg stitches all parts into one timeline.
    let concat_path = outdir.join("concat.txt");
    {
        let mut content = String::new();
        for p in &parts {
            let abs = std::path::Path::new(library_path).join(&p.filename);
            // ffmpeg concat demuxer requires single-quotes escaped as '\'' and
            // the rest of the path double-escaped for the `file` directive.
            let abs_str = abs.to_string_lossy().replace('\'', "'\\''");
            content.push_str(&format!("file '{}'\n", abs_str));
        }
        std::fs::write(&concat_path, &content)?;
    }

    // Signal "transcode started" so a racing status poll sees non-zero progress.
    std::fs::write(progress_path(book_id, profile), "0.1")?;

    let ffmpeg = std::env::var("OMNIBUS_FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into());
    let timeout_secs: u64 = std::env::var("OMNIBUS_HLS_TRANSCODE_TIMEOUT_SECS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1800);

    let seg_pattern = outdir.join("seg-%04d.ts");
    let manifest_out = outdir.join("index.m3u8");

    let result: Result<std::io::Result<std::process::Output>, tokio::time::error::Elapsed> =
        tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
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
                    "-y",
                    &manifest_out.to_string_lossy(),
                ])
                .stderr(std::process::Stdio::piped())
                .spawn()?
                .wait_with_output(),
        )
        .await;

    match result {
        Ok(Ok(output)) if output.status.success() => {
            std::fs::write(progress_path(book_id, profile), "1.0")?;
            // Non-fatal: eviction failure is logged but does not roll back the
            // successful transcode.
            let cap = cap_bytes();
            if let Err(e) = evict_if_over_cap(cap) {
                tracing::warn!(error = %e, "HLS eviction failed after successful transcode");
            }
            Ok(())
        }
        Ok(Ok(output)) => {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::error!(
                book_id = book_id,
                profile = profile,
                status = ?output.status,
                stderr = %stderr,
                "ffmpeg transcode failed"
            );
            cleanup_segment_dir(book_id, profile);
            anyhow::bail!("ffmpeg exited with status {}", output.status)
        }
        Ok(Err(io_err)) => {
            tracing::error!(book_id = book_id, error = %io_err, "ffmpeg spawn/wait error");
            cleanup_segment_dir(book_id, profile);
            Err(io_err.into())
        }
        Err(_timeout) => {
            tracing::error!(
                book_id = book_id,
                profile = profile,
                timeout_secs = timeout_secs,
                "ffmpeg transcode timed out"
            );
            cleanup_segment_dir(book_id, profile);
            anyhow::bail!("ffmpeg transcode timed out after {timeout_secs}s")
        }
    }
}

/// Remove a `(book_id, profile)` segment directory and its sentinel on
/// transcode failure, so a retry starts from a clean slate.
fn cleanup_segment_dir(book_id: i64, profile: &str) {
    let dir = segment_dir(book_id, profile);
    if let Err(e) = std::fs::remove_dir_all(&dir) {
        tracing::warn!(path = ?dir, error = %e, "failed to clean up segment dir after error");
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
mod tests {
    use super::*;

    #[test]
    fn build_manifest_with_zero_duration_emits_stub() {
        let m = build_manifest(&[HlsPart {
            ordinal: 0,
            filename: "track.mp3".into(),
            duration_seconds: 0.0,
        }]);
        assert!(m.contains("#EXTM3U"));
        assert!(m.contains("seg-0000.ts"));
        assert!(m.contains("#EXT-X-ENDLIST"));
    }

    #[test]
    fn build_manifest_calculates_correct_segment_count() {
        // 25 seconds → 3 segments (10, 10, 5).
        let parts = vec![
            HlsPart {
                ordinal: 0,
                filename: "p1.mp3".into(),
                duration_seconds: 15.0,
            },
            HlsPart {
                ordinal: 1,
                filename: "p2.mp3".into(),
                duration_seconds: 10.0,
            },
        ];
        let m = build_manifest(&parts);
        let extinf_count = m.lines().filter(|l| l.starts_with("#EXTINF:")).count();
        assert_eq!(extinf_count, 3, "expected 3 segments for 25 s total");
        // Last segment duration = 25 - 20 = 5.
        assert!(
            m.contains("#EXTINF:5.000,"),
            "last segment should be 5.000 s"
        );
    }

    #[test]
    fn read_progress_returns_zero_when_sentinel_absent() {
        // Use a book_id that will never exist on the test filesystem.
        assert_eq!(read_progress(i64::MAX, AUDIO64), 0.0);
    }

    #[test]
    fn is_ready_returns_false_when_sentinel_absent() {
        assert!(!is_ready(i64::MAX, AUDIO64));
    }
}
