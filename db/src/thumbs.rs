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

/// Failure space for thumbnail generation: a pipeline step failed, the book
/// has no cover to resize, or the DB read failed.
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

/// Default eviction cap (1 GiB). Sized for the lossy encoder (#1750): the
/// 1,635-book dev library holds ~50 MB of thumbnails across all three sizes,
/// so a gigabyte is still ~20× headroom. The pre-#1750 default was 5 GiB
/// because lossless WebP needed it.
const DEFAULT_CAP_BYTES: u64 = 1024 * 1024 * 1024;

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

/// Bump this whenever [`write_thumbnail`]'s encode path changes (a new WebP
/// encoder, a different quality setting, a new output format) so a client
/// holding a validator from the old scheme is forced to re-fetch even though
/// the book's `last_modified_epoch` never moved. This repo doesn't need to
/// detect the encoder version dynamically, so a hand-bumped constant is
/// enough.
///
/// v2 (#1750): lossless `image` WebP → lossy libwebp at
/// [`THUMB_QUALITY`]. A bump also purges the on-disk cache once at boot —
/// see [`purge_stale_scheme_thumbs_once`], which is the other half of this
/// constant's contract: the ETag alone would re-validate a client while the
/// server kept serving the old bytes forever.
const THUMB_ENCODER_VERSION: u32 = 2;

/// Derive a thumbnail's `ETag` from its freshness key — `(book_id, size,
/// last_modified_epoch)`, the exact triple [`is_stale`]/[`is_stale_async`]
/// key freshness on — plus `version`, without touching the filesystem. A
/// validator that cannot disagree with the freshness check it stands in for.
///
/// Deliberately not stat-derived: [`touch_thumb`] bumps the cached file's
/// mtime on every cache-hit read for the LRU in [`evict_if_over_cap`], so an
/// mtime-bearing validator would churn on every request and never produce a
/// 304. Split out from [`thumb_etag`] so a test can vary `version`
/// independently of [`THUMB_ENCODER_VERSION`].
///
/// SHA-256, not `std::hash::Hasher` — the same trap `helpers::stable_uuid`'s
/// doc comment and `kobo::dto`'s fixed-digest derivation both flag:
/// `DefaultHasher`'s algorithm is not guaranteed stable across toolchain
/// versions, so a compiler bump could silently rotate every cached client's
/// ETag and force a mass re-fetch.
fn thumb_etag_versioned(
    book_id: i64,
    size: ThumbSize,
    last_modified_epoch: i64,
    version: u32,
) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(book_id.to_le_bytes());
    hasher.update(size.as_str().as_bytes());
    hasher.update(last_modified_epoch.to_le_bytes());
    hasher.update(version.to_le_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

/// Derive a thumbnail's `ETag` without reading it off disk. See
/// [`thumb_etag_versioned`] for the freshness-key rationale.
pub fn thumb_etag(book_id: i64, size: ThumbSize, last_modified_epoch: i64) -> String {
    thumb_etag_versioned(book_id, size, last_modified_epoch, THUMB_ENCODER_VERSION)
}

/// Marker written into `thumbs_dir()` once its contents are known to come
/// from the current [`THUMB_ENCODER_VERSION`]. Deliberately not a `.webp`,
/// so [`used_bytes`] and [`evict_if_over_cap`] both skip it.
fn scheme_sentinel_name() -> String {
    format!(".omnibus-thumb-scheme-v{THUMB_ENCODER_VERSION}")
}

/// One-time purge of thumbnails written by an older encoder, run at boot.
/// Returns the number of files removed.
///
/// [`is_stale`] compares a thumb's mtime against its book's
/// `last_modified_epoch` and nothing else, so a re-encode at the same source
/// bytes never invalidates anything — an upgraded install would serve its
/// pre-existing lossless thumbs forever. Bumping [`THUMB_ENCODER_VERSION`]
/// rotates the sentinel filename, this sweep sees the new name missing and
/// empties the directory once, and the next request regenerates each thumb
/// through the current encoder.
///
/// Best-effort throughout, like the covers-dir purge in `pool`: a missing
/// directory, an unreadable entry, or a failed unlink is logged and skipped
/// rather than surfaced. The thumbnail directory is a cache, so the worst
/// outcome of a failure is a stale file, never a broken boot.
///
/// **The sentinel lands only on a clean sweep.** Writing it after a partial
/// one would mark the directory current while a previous-scheme file is still
/// sitting in it, and `is_stale` would go on serving that file until its
/// book's `last_modified_epoch` happened to move — the exact
/// serve-it-forever case this function exists to prevent. Leaving the
/// sentinel absent costs a re-sweep of an already-empty directory next boot.
///
/// Must be called inside `tokio::task::spawn_blocking`.
pub fn purge_stale_scheme_thumbs_once() -> usize {
    let dir = thumbs_dir();
    let sentinel = dir.join(scheme_sentinel_name());
    if sentinel.exists() {
        return 0;
    }

    // A directory that doesn't exist yet has nothing to purge, but it still
    // gets marked below — leaving a fresh install unmarked means its *second*
    // boot finds an unmarked directory and throws away the cache the first
    // one just built.
    let mut removed = 0usize;
    let mut left_behind = 0usize;
    if dir.exists() {
        match std::fs::read_dir(&dir) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let path = entry.path();
                    // Files only: nothing writes subdirectories here today,
                    // and a future version that does shouldn't have them
                    // swept out from under it.
                    if !path.is_file() {
                        continue;
                    }
                    match std::fs::remove_file(&path) {
                        Ok(()) => removed += 1,
                        Err(err) => {
                            left_behind += 1;
                            tracing::warn!(
                                path = %path.display(),
                                error = %err,
                                "thumbs: failed to remove previous-scheme thumbnail",
                            );
                        }
                    }
                }
            }
            Err(err) => {
                tracing::warn!(
                    thumbs_dir = %dir.display(),
                    error = %err,
                    "thumbs: could not read dir to purge previous-scheme thumbs; skipping",
                );
                return 0;
            }
        }
    } else if let Err(err) = std::fs::create_dir_all(&dir) {
        tracing::warn!(
            thumbs_dir = %dir.display(),
            error = %err,
            "thumbs: could not create dir to mark the encoder scheme; will retry on next boot",
        );
        return 0;
    }

    if left_behind > 0 {
        tracing::warn!(
            thumbs_dir = %dir.display(),
            removed,
            left_behind,
            "thumbs: previous-scheme purge incomplete; leaving the scheme unmarked so it retries on next boot",
        );
    } else if let Err(err) = std::fs::write(&sentinel, b"\n") {
        tracing::warn!(
            sentinel = %sentinel.display(),
            error = %err,
            "thumbs: failed to write scheme sentinel; will retry on next boot",
        );
    } else {
        tracing::info!(
            thumbs_dir = %dir.display(),
            removed,
            version = THUMB_ENCODER_VERSION,
            "thumbs: purged previous-scheme thumbnails",
        );
    }

    removed
}

/// Lossy WebP quality passed to libwebp, on its 0-100 scale. 80 is the
/// customary photographic setting — visually indistinguishable from lossless
/// on cover art at thumbnail dimensions, at roughly a tenth of the bytes.
/// Changing it means bumping [`THUMB_ENCODER_VERSION`].
const THUMB_QUALITY: f32 = 80.0;

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
    use image::imageops::FilterType;

    let (w, h) = size.dimensions();
    // `resize_to_fill` guarantees output dimensions equal `(w, h)` by
    // resizing-then-cropping. Plain `resize` preserves aspect ratio and can
    // return a smaller image, which would defeat the frontend's fixed
    // `width`/`height` attributes and stretch covers.
    let resized = decoded.resize_to_fill(w, h, FilterType::Lanczos3);

    // RGBA rather than RGB: a cover with transparency would otherwise have
    // its alpha channel dropped without blending, painting whatever garbage
    // sits under the transparent pixels. libwebp drops the alpha plane by
    // itself when the image turns out to be fully opaque, so the common case
    // costs nothing.
    let rgba = resized.to_rgba8();
    let webp_bytes = webp::Encoder::from_rgba(rgba.as_raw(), w, h)
        .encode_simple(false, THUMB_QUALITY)
        .map_err(|e| ThumbError::Failed(format!("WebP encode failed: {e:?}")))?
        .to_vec();

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

/// Total bytes currently held by the thumbnail cache — every regular
/// `.webp` file directly under `thumbs_dir()`. Read-only counterpart to
/// [`evict_if_over_cap`]'s usage computation, for the admin health page's
/// storage section. Missing directory or an unreadable entry reads as
/// empty/skipped rather than an error — this is a display figure, not an
/// eviction decision.
pub fn used_bytes() -> u64 {
    let dir = thumbs_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".webp"))
        .filter_map(|entry| entry.metadata().ok())
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .sum()
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
