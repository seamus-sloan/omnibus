//! Filesystem cover I/O. Covers live under
//! `<OMNIBUS_COVERS_DIR>/<uuid>.<ext>` so a backup of the SQLite DB stays
//! small and covers can be regenerated independently by reindexing.
//! `books.has_cover` tracks whether a file should exist; a missing file on
//! disk is treated as "no cover" (404), not an error.

use std::collections::HashMap;
use std::path::PathBuf;

use sqlx::SqlitePool;

/// Errors returned by the on-disk cover read path.
///
/// Today the only failure mode that surfaces here is the DB lookup that
/// resolves a book id to its uuid + cover flags. Filesystem probes
/// (`find_cover_file` / `find_override_cover_file`) swallow every
/// `std::fs::read` failure and return `Ok(None)` — they don't distinguish
/// a missing file from a permissions or I/O error.
#[derive(Debug, thiserror::Error)]
pub enum CoversError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

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

    /// Map the format `image::guess_format` sniffed from the bytes onto our
    /// local set, or `None` for a codec we don't cache (SVG isn't a raster
    /// format `image` sniffs, and anything else falls back to the mime).
    fn from_guessed(guessed: image::ImageFormat) -> Option<Self> {
        match guessed {
            image::ImageFormat::Jpeg => Some(ImageFormat::Jpeg),
            image::ImageFormat::Png => Some(ImageFormat::Png),
            image::ImageFormat::Gif => Some(ImageFormat::Gif),
            image::ImageFormat::WebP => Some(ImageFormat::Webp),
            _ => None,
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
    // Trust the bytes over the caller-supplied mime: a cover mislabelled
    // `image/jpeg` whose bytes are really a GIF must land as `<uuid>.gif`,
    // not `<uuid>.jpg` (#828). Fall back to the mime only when the sniff
    // fails or yields a format outside our local set (e.g. SVG).
    let fmt = image::guess_format(bytes)
        .ok()
        .and_then(ImageFormat::from_guessed)
        .unwrap_or_else(|| ImageFormat::from_mime(mime));
    std::fs::write(cover_path_for(uuid, fmt.to_ext()), bytes)
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

/// Cheaply resolve a book's cover MIME type by checking which known
/// extension exists on disk — unlike [`get_cover`] / [`find_cover_file`],
/// no file bytes are read, only a handful of `stat`-class existence checks.
/// Meant for callers (the OPDS catalog) that need an accurate `type`
/// attribute on a cover link without paying for the full read that actually
/// serving the file requires. Checks the override path first when
/// `has_cover_override` is set, matching [`get_cover`]'s precedence.
///
/// Falls back to `image/jpeg` — the common case, and what every OPDS entry
/// advertised before this existed — when no cover file is found under any
/// known extension; the byte-serving endpoint 404s in that case regardless,
/// so the advertised type is moot.
///
/// Deliberately synchronous `std::fs` (unlike [`get_cover`]'s
/// `spawn_blocking`-wrapped probes): callers skip it entirely for a book
/// with no `cover_url` (the common no-cover case), and an OPDS page is
/// capped at a bounded book count — a few extra `stat`s per row is not the
/// kind of hot loop `spawn_blocking`'s dispatch overhead is worth paying
/// for. Revisit if a caller ever calls this outside that bound.
pub fn cover_mime_hint(uuid: &str, has_cover_override: bool) -> &'static str {
    let dir = covers_dir();
    if has_cover_override {
        for fmt in ImageFormat::PROBE_ORDER {
            if dir
                .join(format!("override-{uuid}.{}", fmt.to_ext()))
                .is_file()
            {
                return fmt.to_mime();
            }
        }
    }
    for fmt in ImageFormat::PROBE_ORDER {
        if dir.join(format!("{uuid}.{}", fmt.to_ext())).is_file() {
            return fmt.to_mime();
        }
    }
    ImageFormat::Jpeg.to_mime()
}

/// Load a book's cover image bytes + mime type from disk. The `id` parameter
/// is the `books.id` primary key — the REST surface is uuid-keyed at
/// `/api/covers/{uuid}` (`server/src/backend.rs`), where the handler resolves
/// the uuid to an id via `resolve_book_id_by_uuid` before calling this.
///
/// User-uploaded override covers take precedence: when the
/// `metadata_overrides` table flags `has_cover_override`, the override file
/// at `covers_dir()/override-<uuid>.<ext>` is returned first. The override
/// flag is pulled in via a `LEFT JOIN` so the hot path stays at one query.
///
/// Filesystem probes (`find_override_cover_file` / `find_cover_file`) are
/// synchronous `std::fs` calls and run on the blocking pool via
/// [`tokio::task::spawn_blocking`] so a hot cover-fetch loop doesn't pin
/// tokio worker threads.
pub async fn get_cover(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<(String, Vec<u8>)>, CoversError> {
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
                error = %join_err,
                "get_cover spawn_blocking {kind}"
            );
            None
        }
    };
    Ok(result)
}

/// Return `books.last_modified` (INTEGER unix-seconds since migration 0038) for
/// `book_id`, or `None` if the book does not exist. `last_modified` is nullable
/// after the in-place 0038 conversion, so a row that somehow lacks one falls
/// back to now — a bare `i64` decode of a NULL would error and 500 the thumbs
/// endpoint; falling back keeps it serving and regenerates the stale thumbnail.
pub async fn get_last_modified_epoch(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<i64>, CoversError> {
    // `strftime` returns TEXT and a SELECT has no column affinity to coerce it,
    // so CAST the fallback back to INTEGER for the `i64` decode.
    Ok(sqlx::query_scalar(
        "SELECT CAST(COALESCE(last_modified, strftime('%s','now')) AS INTEGER) FROM books WHERE id = ?",
    )
    .bind(book_id)
    .fetch_optional(pool)
    .await?)
}

/// Bulk counterpart to [`get_last_modified_epoch`]: resolve `books.last_modified`
/// for every id in `ids` in a handful of chunked `IN (...)` queries instead of
/// one round trip per book. Same NULL-falls-back-to-now behavior; ids with no
/// matching row are simply absent from the returned map.
pub(crate) async fn last_modified_bulk(
    pool: &SqlitePool,
    ids: &[i64],
) -> Result<HashMap<i64, i64>, CoversError> {
    let mut map = HashMap::with_capacity(ids.len());
    for chunk in ids.chunks(499) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, CAST(COALESCE(last_modified, strftime('%s','now')) AS INTEGER) \
             FROM books WHERE id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, i64)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (id, last_modified) in q.fetch_all(pool).await? {
            map.insert(id, last_modified);
        }
    }
    Ok(map)
}

#[cfg(test)]
mod tests;
