//! Filesystem layout for the KEPUB cache: cache dir, per-book path, and the
//! `books.last_modified`-driven staleness check. Mirrors the thumbnail cache
//! (`crate::thumbs`).

use std::path::PathBuf;
use std::time::UNIX_EPOCH;

/// Root directory for the KEPUB cache.
///
/// Override with `$OMNIBUS_KEPUB_DIR` (used verbatim); otherwise defaults to
/// `<$OMNIBUS_DATA_DIR>/kepub` (data dir default `./data`).
pub fn kepub_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_KEPUB_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("kepub")
}

/// Cache path for one book's converted KEPUB:
/// `<kepub_dir>/<book_id>.kepub.epub`.
pub fn kepub_path(book_id: i64) -> PathBuf {
    kepub_dir().join(format!("{book_id}.kepub.epub"))
}

/// `mtime` of `meta` as epoch seconds, or `0` if unavailable.
fn mtime_epoch(meta: &std::fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// `true` when the cached KEPUB for `book_id` is missing or older than the
/// book's `last_modified` (so a metadata/file edit forces a reconvert).
/// Identical semantics to `thumbs::is_stale`.
pub fn is_stale(book_id: i64, last_modified_epoch: i64) -> bool {
    let path = kepub_path(book_id);
    match std::fs::metadata(&path) {
        Err(_) => true,
        Ok(meta) => mtime_epoch(&meta) < last_modified_epoch,
    }
}

/// Async form of [`is_stale`] for callers on a tokio runtime — uses
/// `tokio::fs::metadata` so it doesn't block a worker thread (mirrors
/// `thumbs::is_stale_async`).
pub async fn is_stale_async(book_id: i64, last_modified_epoch: i64) -> bool {
    let path = kepub_path(book_id);
    match tokio::fs::metadata(&path).await {
        Err(_) => true,
        Ok(meta) => mtime_epoch(&meta) < last_modified_epoch,
    }
}
