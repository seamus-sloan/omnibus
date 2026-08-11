//! Filesystem layout for the format-conversion output cache: the cache
//! directory and the per-`(book_id, target_format)` output path. Mirrors
//! `crate::kepub::fs`.

use std::path::PathBuf;

/// Root directory for converted-format output files.
///
/// Override with `$OMNIBUS_CONVERT_DIR` (used verbatim); otherwise defaults
/// to `<$OMNIBUS_DATA_DIR>/convert` (data dir default `./data`).
pub fn convert_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_CONVERT_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("convert")
}

/// Output path for one book's conversion to `target_format`:
/// `<convert_dir>/<book_id>.<target_format lowercased>`.
pub fn convert_path(book_id: i64, target_format: &str) -> PathBuf {
    convert_dir().join(format!("{book_id}.{}", target_format.to_lowercase()))
}
