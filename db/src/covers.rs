//! Filesystem cover I/O. Covers live under
//! `<OMNIBUS_COVERS_DIR>/<uuid>.<ext>` so a backup of the SQLite DB stays
//! small and covers can be regenerated independently by reindexing.
//! `books.has_cover` tracks whether a file should exist; a missing file on
//! disk is treated as "no cover" (404), not an error.
//!
//! Also hosts the `CoversTempDir` test guard used throughout the db tests
//! — `pub(crate)` so siblings can `use crate::covers::test_helpers::*`
//! without each test module needing its own copy of the OMNIBUS_COVERS_DIR
//! mutex.

use std::path::PathBuf;

use sqlx::SqlitePool;

/// Root directory for cover files. Override with `OMNIBUS_COVERS_DIR`.
pub fn covers_dir() -> PathBuf {
    std::env::var("OMNIBUS_COVERS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./covers"))
}

/// Image formats we know how to round-trip through the on-disk cover cache.
/// `Svg` sticks around because some EPUB covers ship as SVG; `Bin` is the
/// fallback extension for unknown bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ImageFormat {
    Jpeg,
    Png,
    Gif,
    Webp,
    Svg,
    Bin,
}

impl ImageFormat {
    pub(crate) fn to_mime(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "image/jpeg",
            ImageFormat::Png => "image/png",
            ImageFormat::Gif => "image/gif",
            ImageFormat::Webp => "image/webp",
            ImageFormat::Svg => "image/svg+xml",
            ImageFormat::Bin => "application/octet-stream",
        }
    }

    pub(crate) fn to_ext(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
            ImageFormat::Gif => "gif",
            ImageFormat::Webp => "webp",
            ImageFormat::Svg => "svg",
            ImageFormat::Bin => "bin",
        }
    }

    pub(crate) fn from_mime(mime: &str) -> Self {
        match mime.to_ascii_lowercase().as_str() {
            "image/jpeg" | "image/jpg" => ImageFormat::Jpeg,
            "image/png" => ImageFormat::Png,
            "image/gif" => ImageFormat::Gif,
            "image/webp" => ImageFormat::Webp,
            "image/svg+xml" => ImageFormat::Svg,
            _ => ImageFormat::Bin,
        }
    }

    pub(crate) fn from_ext(ext: &str) -> Self {
        match ext.to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => ImageFormat::Jpeg,
            "png" => ImageFormat::Png,
            "gif" => ImageFormat::Gif,
            "webp" => ImageFormat::Webp,
            "svg" => ImageFormat::Svg,
            _ => ImageFormat::Bin,
        }
    }

    /// Extensions probed in `find_cover_file`, ordered by how likely each is
    /// to be the on-disk format. Keeping it on the type means adding a new
    /// variant only requires updating the match arms above plus this list.
    pub(crate) const PROBE_ORDER: [ImageFormat; 6] = [
        ImageFormat::Jpeg,
        ImageFormat::Png,
        ImageFormat::Webp,
        ImageFormat::Gif,
        ImageFormat::Svg,
        ImageFormat::Bin,
    ];
}

pub(crate) fn cover_path_for(uuid: &str, ext: &str) -> PathBuf {
    covers_dir().join(format!("{uuid}.{ext}"))
}

pub(crate) fn write_cover_file(uuid: &str, mime: &str, bytes: &[u8]) -> std::io::Result<()> {
    let dir = covers_dir();
    std::fs::create_dir_all(&dir)?;
    let ext = ImageFormat::from_mime(mime).to_ext();
    std::fs::write(cover_path_for(uuid, ext), bytes)
}

pub(crate) fn find_cover_file(uuid: &str) -> Option<(String, Vec<u8>)> {
    // Try common extensions in the order covers are most likely to be
    // written. Fall back to a directory scan for `<uuid>.*` if none match,
    // so migrations that introduce new extensions don't require a code
    // change here.
    for fmt in ImageFormat::PROBE_ORDER {
        let p = cover_path_for(uuid, fmt.to_ext());
        if let Ok(bytes) = std::fs::read(&p) {
            return Some((fmt.to_mime().to_string(), bytes));
        }
    }
    // Fallback scan.
    let dir = covers_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(dot) = name_str.rfind('.') {
                let (stem, ext) = name_str.split_at(dot);
                if stem == uuid {
                    if let Ok(bytes) = std::fs::read(entry.path()) {
                        let mime = ImageFormat::from_ext(&ext[1..]).to_mime();
                        return Some((mime.to_string(), bytes));
                    }
                }
            }
        }
    }
    None
}

pub(crate) fn delete_cover_files_for(uuids: &[String]) {
    for uuid in uuids {
        for fmt in ImageFormat::PROBE_ORDER {
            let _ = std::fs::remove_file(cover_path_for(uuid, fmt.to_ext()));
        }
    }
}

/// Probe for a user-uploaded override cover file for the given UUID.
pub(crate) fn find_override_cover_file(uuid: &str) -> Option<(String, Vec<u8>)> {
    let dir = covers_dir();
    for fmt in ImageFormat::PROBE_ORDER {
        let path = dir.join(format!("override-{uuid}.{}", fmt.to_ext()));
        if let Ok(bytes) = std::fs::read(&path) {
            return Some((fmt.to_mime().to_string(), bytes));
        }
    }
    None
}

/// Load a book's cover image bytes + mime type from disk. The `id` parameter
/// is the `books.id` primary key (so the `/api/covers/:id` URL shape stays
/// stable); internally we look up the book's `uuid` and read the file.
///
/// **F5.1:** User-uploaded override covers take precedence. When the
/// `metadata_overrides` table flags `has_cover_override`, the override file
/// at `covers_dir()/override-<uuid>.<ext>` is returned first.
///
/// Single round-trip: the override flag is pulled in via a `LEFT JOIN` on
/// `metadata_overrides` rather than a second `get_metadata_overrides` call.
/// Covers are fetched per grid tile and per detail page — the hot path
/// stays at one query regardless of whether overrides exist.
///
/// The filesystem probes (`find_override_cover_file` / `find_cover_file`)
/// are synchronous `std::fs` calls and run on the blocking pool via
/// [`tokio::task::spawn_blocking`] so a hot cover-fetch loop doesn't pin
/// tokio worker threads (#106).
pub async fn get_cover(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    // `COALESCE(mo.has_cover_override, 0)` keeps the flag at 0 when no
    // override row exists (the LEFT JOIN yields NULL in that case), so the
    // row tuple can decode into `i64` instead of `Option<i64>`.
    let row: Option<(String, i64, i64)> = sqlx::query_as(
        "SELECT b.uuid, b.has_cover, COALESCE(mo.has_cover_override, 0)
           FROM books b
           LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
          WHERE b.id = ?",
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await?;

    let Some((uuid, has_cover, has_cover_override)) = row else {
        return Ok(None);
    };

    // F5.1: check for override cover first.
    let has_cover_override = has_cover_override != 0;

    // Move the sync `std::fs` probes off the runtime. `JoinError` (panic or
    // cancellation) can't round-trip through `sqlx::Error`, so we fold it
    // into "no cover" — but log it loudly first so a real panic doesn't get
    // silently masked into a missing-cover symptom.
    let uuid_for_blocking = uuid.clone();
    let result = match tokio::task::spawn_blocking(move || {
        if has_cover_override {
            if let Some(cover) = find_override_cover_file(&uuid_for_blocking) {
                return Some(cover);
            }
        }
        if has_cover != 0 {
            find_cover_file(&uuid_for_blocking)
        } else {
            None
        }
    })
    .await
    {
        Ok(cover) => cover,
        Err(join_err) => {
            let kind = if join_err.is_panic() {
                "panicked"
            } else {
                "was cancelled"
            };
            tracing::error!(
                book_id,
                uuid = %uuid,
                "get_cover spawn_blocking {kind}: {join_err}"
            );
            None
        }
    };
    Ok(result)
}

/// Return `strftime('%s', last_modified)` as epoch seconds for `book_id`,
/// or `None` if the book does not exist.
pub async fn get_last_modified_epoch(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT CAST(strftime('%s', last_modified) AS INTEGER) FROM books WHERE id = ?",
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await
}

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Shared test guards used by every db test module that touches the
    //! covers directory. `OMNIBUS_COVERS_DIR` is a process-global env var,
    //! so tests that touch it must serialize. A single Mutex held for the
    //! duration of each test keeps parallel `cargo test` runs from stomping
    //! on each other's covers dir.

    use std::path::PathBuf;

    pub(crate) static COVERS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    pub(crate) struct CoversTempDir {
        pub(crate) path: PathBuf,
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl CoversTempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let guard = COVERS_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pid = std::process::id();
            let seq = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("omnibus_covers_{tag}_{pid}_{seq}"));
            let _ = std::fs::remove_dir_all(&path);
            let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
            std::env::set_var("OMNIBUS_COVERS_DIR", &path);
            Self {
                path,
                prev,
                _guard: guard,
            }
        }
    }

    impl Drop for CoversTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
            match self.prev.take() {
                Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
                None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
            }
        }
    }
}
