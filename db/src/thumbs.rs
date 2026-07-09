//! Thumbnail pipeline — generation, caching, and eviction.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Thumbnail sizes for cover images.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ThumbSize {
    Sm,
    Md,
    Lg,
}

impl ThumbSize {
    /// Pixel dimensions (width, height) at 2:3 aspect ratio.
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            ThumbSize::Sm => (160, 240),
            ThumbSize::Md => (320, 480),
            ThumbSize::Lg => (640, 960),
        }
    }

    /// Returns the string key for this thumbnail size variant (`"sm"`, `"md"`, or `"lg"`).
    pub fn as_str(self) -> &'static str {
        match self {
            ThumbSize::Sm => "sm",
            ThumbSize::Md => "md",
            ThumbSize::Lg => "lg",
        }
    }

    /// Returns a fixed-size array of all `ThumbSize` variants in ascending size order.
    pub fn all() -> [ThumbSize; 3] {
        [ThumbSize::Sm, ThumbSize::Md, ThumbSize::Lg]
    }
}

impl std::fmt::Display for ThumbSize {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ThumbSize {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "sm" => Ok(ThumbSize::Sm),
            "md" => Ok(ThumbSize::Md),
            "lg" => Ok(ThumbSize::Lg),
            _ => Err(()),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ThumbError {
    /// Decode, encode, or I/O failure in the pipeline; the message names the step.
    #[error("thumbnail generation failed: {0}")]
    Failed(String),
    #[error("no cover available for book {0}")]
    NoCover(i64),
    #[error("db error: {0}")]
    Db(#[from] sqlx::Error),
}

/// Default eviction cap (5 GiB).
const DEFAULT_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Root directory for thumbnail files. Override with `OMNIBUS_THUMBS_DIR`.
pub fn thumbs_dir() -> PathBuf {
    std::env::var("OMNIBUS_THUMBS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./thumbs"))
}

/// Resolved eviction cap in bytes. Reads `OMNIBUS_THUMBS_CAP_BYTES`; falls
/// back to [`DEFAULT_CAP_BYTES`] when unset or unparseable.
pub fn cap_bytes() -> u64 {
    std::env::var("OMNIBUS_THUMBS_CAP_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CAP_BYTES)
}

/// Full on-disk path: `<thumbs_dir>/<book_id>_<size>.webp`
pub fn thumb_path_for(book_id: i64, size: ThumbSize) -> PathBuf {
    thumbs_dir().join(format!("{book_id}_{size}.webp"))
}

fn mtime_epoch(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// True if the cached thumbnail is absent or no newer than
/// `last_modified_epoch` (Unix seconds). Synchronous variant — call from
/// `spawn_blocking` contexts (the worker's encode loop). For the async
/// request path, use [`is_stale_async`] so the metadata syscall doesn't pin a
/// tokio worker.
///
/// Uses `<=` rather than `<`: both timestamps are whole-second Unix epochs,
/// so a thumb regenerated in the same wall-clock second as a cover rewrite
/// would otherwise tie and be treated as fresh.
pub fn is_stale(book_id: i64, size: ThumbSize, last_modified_epoch: i64) -> bool {
    let path = thumb_path_for(book_id, size);
    match std::fs::metadata(&path) {
        Err(_) => true,
        Ok(meta) => mtime_epoch(&meta) <= last_modified_epoch,
    }
}

/// Async variant of [`is_stale`] for the request path.
pub async fn is_stale_async(book_id: i64, size: ThumbSize, last_modified_epoch: i64) -> bool {
    let path = thumb_path_for(book_id, size);
    match tokio::fs::metadata(&path).await {
        Err(_) => true,
        Ok(meta) => mtime_epoch(&meta) <= last_modified_epoch,
    }
}

/// Resize a pre-decoded cover and write the WebP to disk for one size.
///
/// Atomic on POSIX: the WebP is written to a per-(book,size) temp file in
/// `thumbs_dir()` and then `rename`d into place, so a concurrent reader can
/// never observe a partial file.
fn write_thumbnail(
    book_id: i64,
    size: ThumbSize,
    decoded: &image::DynamicImage,
) -> Result<usize, ThumbError> {
    use image::{imageops::FilterType, ImageFormat};

    let (w, h) = size.dimensions();
    // `resize_to_fill` guarantees output dimensions equal `(w, h)` by
    // resizing-then-cropping. Plain `resize` preserves aspect ratio and can
    // return a smaller image, which would defeat the frontend's fixed
    // `width`/`height` attributes and stretch covers.
    let resized = decoded.resize_to_fill(w, h, FilterType::Lanczos3);

    let webp_bytes = {
        let mut buf = std::io::Cursor::new(Vec::new());
        resized
            .write_to(&mut buf, ImageFormat::WebP)
            .map_err(|e| ThumbError::Failed(format!("WebP encode failed: {e}")))?;
        buf.into_inner()
    };

    let dir = thumbs_dir();
    std::fs::create_dir_all(&dir).map_err(|e| ThumbError::Failed(format!("I/O error: {e}")))?;

    let final_path = thumb_path_for(book_id, size);
    // Per-(book,size) temp name keeps concurrent generations from clobbering
    // each other's temp files. The worker's `thumb:{book_id}` resource lock
    // already serializes per-book, so a single suffix is enough.
    let tmp_path = dir.join(format!("{book_id}_{size}.webp.tmp"));
    std::fs::write(&tmp_path, &webp_bytes)
        .map_err(|e| ThumbError::Failed(format!("I/O error: {e}")))?;
    std::fs::rename(&tmp_path, &final_path)
        .map_err(|e| ThumbError::Failed(format!("I/O error: {e}")))?;

    Ok(webp_bytes.len())
}

/// Generate one thumbnail size from raw cover bytes and write to disk.
///
/// Must be called inside `tokio::task::spawn_blocking` — decode + encode are
/// CPU-bound. Prefer [`ensure_thumbnails_sync`] when generating multiple
/// sizes for the same cover, since it decodes the source image only once.
pub fn generate_thumbnail(
    book_id: i64,
    size: ThumbSize,
    cover_bytes: &[u8],
) -> Result<usize, ThumbError> {
    let decoded = image::load_from_memory(cover_bytes)
        .map_err(|e| ThumbError::Failed(format!("cover decode failed: {e}")))?;
    write_thumbnail(book_id, size, &decoded)
}

/// Ensure all three thumbnail sizes are generated and fresh.
///
/// Decodes `cover_bytes` once and reuses the [`image::DynamicImage`] across
/// every size that's currently stale, then writes each WebP atomically.
///
/// Must be called inside `tokio::task::spawn_blocking`.
pub fn ensure_thumbnails_sync(
    book_id: i64,
    last_modified_epoch: i64,
    cover_bytes: Vec<u8>,
) -> Result<(), ThumbError> {
    let mut decoded: Option<image::DynamicImage> = None;
    for size in ThumbSize::all() {
        if !is_stale(book_id, size, last_modified_epoch) {
            continue;
        }
        let img = match decoded.as_ref() {
            Some(img) => img,
            None => decoded.insert(
                image::load_from_memory(&cover_bytes)
                    .map_err(|e| ThumbError::Failed(format!("cover decode failed: {e}")))?,
            ),
        };
        write_thumbnail(book_id, size, img)?;
    }
    Ok(())
}

/// Delete all cached thumbnails for a book so the next request regenerates
/// them. Called after a cover override upload so stale thumbs don't linger.
pub fn invalidate_thumbs(book_id: i64) {
    for size in ThumbSize::all() {
        let _ = std::fs::remove_file(thumb_path_for(book_id, size));
    }
}

/// Bump a cached thumbnail's mtime to now, marking it recently-used for
/// [`evict_if_over_cap`]'s LRU ordering. Call on every cache-hit read (the
/// request path calls this via `spawn_blocking`, fire-and-forget, so it
/// never adds latency to the response). Best-effort: a failure (e.g. the
/// file was evicted between the freshness check and this call) is not worth
/// surfacing — the file is simply gone, so there's nothing left to touch.
pub fn touch_thumb(book_id: i64, size: ThumbSize) {
    if let Ok(file) = std::fs::File::open(thumb_path_for(book_id, size)) {
        let _ = file.set_modified(SystemTime::now());
    }
}

/// Walk `thumbs_dir()` and delete files in oldest-mtime-first order until the
/// total cache size is under `cap_bytes`. [`touch_thumb`] bumps mtime on
/// every cache-hit read, so this is LRU in effect, not pure FIFO-by-creation.
///
/// Must be called inside `tokio::task::spawn_blocking`.
pub fn evict_if_over_cap(cap_bytes: u64) -> Result<(), std::io::Error> {
    let dir = thumbs_dir();
    if !dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    let mut total: u64 = 0;

    for entry in std::fs::read_dir(&dir)?.flatten() {
        if !entry.file_name().to_string_lossy().ends_with(".webp") {
            continue;
        }
        let meta = entry.metadata()?;
        let size = meta.len();
        let mtime = meta.modified().unwrap_or(UNIX_EPOCH);
        total += size;
        entries.push((mtime, entry.path(), size));
    }

    if total <= cap_bytes {
        return Ok(());
    }

    entries.sort_by_key(|(mtime, _, _)| *mtime);
    for (_, path, size) in &entries {
        if total <= cap_bytes {
            break;
        }
        // Only credit the eviction if the delete actually succeeded —
        // otherwise the cache is still over-cap and we shouldn't lie to
        // the running total. Silent failures (e.g. a concurrent reader
        // holding the file open on Windows) get logged so they don't
        // disappear, but we keep going so a single bad file can't block
        // freeing the rest.
        match std::fs::remove_file(path) {
            Ok(()) => total = total.saturating_sub(*size),
            Err(e) => tracing::warn!(
                error = %e,
                path = ?path,
                "thumbs: evict failed"
            ),
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
