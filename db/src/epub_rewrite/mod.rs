//! Bake a book's effective metadata + cover override *into* a copy of its
//! EPUB, so exports (Send-to-Kobo KEPUB, plain download) carry the user's
//! edits instead of shipping the untouched source file. This writes
//! inside the `.epub` container, which is the only metadata a Kobo / kepubify
//! actually reads.
//!
//! One-shot and cache-backed: the rewritten EPUB is cached at
//! `<export dir>/<book_id>.epub`, invalidated on `books.last_modified` (bumped
//! whenever an override is saved), exactly like the thumbnail / KEPUB caches.
//! A book with no active overrides yields `None` — callers serve the source
//! file byte-for-byte.

mod archive;
mod cover;
mod opf;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context;
use omnibus_shared::{BulkRewriteError, BulkRewriteSummary, EbookMetadata};
use sqlx::SqlitePool;

use archive::rewrite_archive;
use cover::encode_cover_for;
use opf::transform_opf;

/// Monotonic counter making each rewrite's temp filename unique within the
/// process, so concurrent (non-serialized) rewrites of the same book can't
/// clobber each other's temp file before the final rename.
static TMP_SEQ: AtomicU64 = AtomicU64::new(0);

/// Errors from the export-rewrite path.
///
/// `BookNotFound` is the one state a caller branches on (404); DB lookups
/// surface via `Books`, and every foreign-system failure (zip, XML, image,
/// filesystem, blocking-join) collapses into `Failed` — the caller only ever
/// falls back to serving the source EPUB, so a finer split buys nothing.
#[derive(Debug, thiserror::Error)]
pub enum EpubRewriteError {
    #[error("book {0} not found")]
    BookNotFound(i64),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("epub rewrite failed: {0}")]
    Failed(#[from] anyhow::Error),
}

/// Root directory for the rewritten-EPUB export cache.
///
/// Override with `$OMNIBUS_EXPORT_EPUB_DIR` (used verbatim); otherwise defaults
/// to `<$OMNIBUS_DATA_DIR>/export-epub` (data dir default `./data`). Mirrors
/// `kepub::kepub_dir`.
pub fn export_epub_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_EXPORT_EPUB_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("export-epub")
}

/// Cache path for one book's rewritten EPUB: `<export dir>/<book_id>.epub`.
///
/// `pub(crate)` (rather than private) so sibling-module tests — e.g.
/// `metadata_overrides`'s cache-invalidation tests — can seed/assert against
/// the exact path without duplicating this naming scheme.
pub(crate) fn export_epub_path(book_id: i64) -> PathBuf {
    export_epub_dir().join(format!("{book_id}.epub"))
}

/// Default eviction cap (5 GiB), mirroring `thumbs`/`hls`.
const DEFAULT_CAP_BYTES: u64 = 5 * 1024 * 1024 * 1024;

/// Resolved eviction cap in bytes. Reads `OMNIBUS_EXPORT_EPUB_CAP_BYTES`;
/// falls back to [`DEFAULT_CAP_BYTES`] when unset or unparseable.
pub fn cap_bytes() -> u64 {
    std::env::var("OMNIBUS_EXPORT_EPUB_CAP_BYTES")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CAP_BYTES)
}

/// Delete the cached rewritten EPUB for `book_id`, if present.
///
/// Called once a book's override state fully clears — see
/// [`crate::metadata_overrides::delete_metadata_overrides`] and
/// [`crate::metadata_overrides::clear_cover_override`] — and when the book
/// itself is deleted (`deletion::fs`), so the cache doesn't linger
/// unreferenced (#1395). A missing file is a no-op; any other failure is
/// logged and swallowed — the caller has already decided the cache shouldn't
/// exist, and a stray file is a disk-space nuisance, not a correctness
/// problem.
pub fn invalidate_export_epub_cache(book_id: i64) {
    if let Err(e) = std::fs::remove_file(export_epub_path(book_id)) {
        if e.kind() != std::io::ErrorKind::NotFound {
            tracing::warn!(book_id, error = %e, "export_epub: failed to remove stale cache file");
        }
    }
}

/// Walk `export_epub_dir()` and delete cached rewritten EPUBs, oldest-mtime
/// first, until the total cache size is under `cap_bytes`.
///
/// Unlike [`crate::thumbs::evict_if_over_cap`] there is no cache-hit mtime
/// touch, so eviction order is FIFO by last-rewrite time rather than true
/// LRU — acceptable here since this cache is written only on an explicit
/// export/download action, not on every page view.
///
/// Must be called inside `tokio::task::spawn_blocking`.
pub fn evict_if_over_cap(cap_bytes: u64) -> Result<(), std::io::Error> {
    let dir = export_epub_dir();
    if !dir.exists() {
        return Ok(());
    }

    let mut entries: Vec<(SystemTime, PathBuf, u64)> = Vec::new();
    let mut total: u64 = 0;

    for entry in std::fs::read_dir(&dir)?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Skip in-flight temp files (`.{book_id}.{pid}.{seq}.tmp.epub`) — not
        // yet promoted cache entries, and may still be actively written.
        if name.starts_with('.') || !name.ends_with(".epub") {
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
        match std::fs::remove_file(path) {
            Ok(()) => total = total.saturating_sub(*size),
            Err(e) => tracing::warn!(error = %e, path = ?path, "export_epub: evict failed"),
        }
    }

    Ok(())
}

/// Path to the EPUB that should be exported for `book_id`, given its `source`
/// EPUB on disk.
///
/// Returns `Ok(None)` when the book has no active metadata/cover override — the
/// caller then serves `source` unchanged (byte-faithful passthrough). Returns
/// `Ok(Some(path))` to a cached rewritten EPUB otherwise, rebuilding it when
/// stale vs. `books.last_modified`. Idempotent: a fresh cache is returned
/// without touching the filesystem.
pub async fn rewritten_epub_path(
    pool: &SqlitePool,
    book_id: i64,
    source: &Path,
) -> Result<Option<PathBuf>, EpubRewriteError> {
    let book = crate::books::get_book(pool, book_id)
        .await?
        .ok_or(EpubRewriteError::BookNotFound(book_id))?;

    // Effective state already reflects the library's metadata-source precedence
    // (an admin who ranks embedded tags above overrides gets no rewrite).
    // Nothing to bake → source.
    if !book.has_override && !book.has_cover_override {
        // Belt-and-suspenders cleanup (#1395): the write paths that clear
        // overrides (`delete_metadata_overrides`, `clear_cover_override`)
        // already invalidate this cache eagerly, so this normally finds
        // nothing. It still catches any cache file left behind from before
        // that fix shipped, or any future path that clears overrides without
        // going through those two functions.
        if let Err(e) =
            tokio::task::spawn_blocking(move || invalidate_export_epub_cache(book_id)).await
        {
            tracing::warn!(book_id, error = %e, "export_epub: cache-invalidate task join failed");
        }
        return Ok(None);
    }

    let out = export_epub_path(book_id);
    let last_modified = crate::get_last_modified_epoch(pool, book_id)
        .await
        .map_err(|e| EpubRewriteError::Failed(anyhow::Error::new(e)))?
        .unwrap_or(0);
    if !is_stale(&out, last_modified).await {
        return Ok(Some(out));
    }

    let dir = export_epub_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create export-epub dir {}", dir.display()))?;
    // Per-call temp name: the download route isn't worker-serialized, so two
    // concurrent rewrites of the same book must not share a temp file (one
    // would truncate the other mid-write). Each writes its own, then the final
    // atomic rename is a harmless last-writer-wins — the content is identical.
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".{book_id}.{}.{seq}.tmp.epub", std::process::id()));

    let src = source.to_path_buf();
    let tmp_for_task = tmp.clone();
    let rewrite = tokio::task::spawn_blocking(move || rewrite_blocking(&src, &tmp_for_task, &book))
        .await
        .map_err(|e| EpubRewriteError::Failed(anyhow::Error::new(e)))?;
    if let Err(e) = rewrite {
        // Don't leave a half-written temp behind on a rewrite failure.
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(e.into());
    }

    // Atomic promote so a concurrent reader never sees a torn file.
    tokio::fs::rename(&tmp, &out)
        .await
        .context("promote rewritten epub into cache")?;

    // Best-effort, non-fatal: the freshly rewritten file above is already
    // promoted, so a failed/skipped eviction only leaves the cache
    // temporarily over its cap (mirrors `thumbs`/`hls`'s eviction hook).
    let cap = cap_bytes();
    match tokio::task::spawn_blocking(move || evict_if_over_cap(cap)).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "export_epub: eviction failed"),
        Err(e) => tracing::warn!(error = %e, "export_epub: eviction task join failed"),
    }
    Ok(Some(out))
}

/// Synchronous rewrite (zip + image codecs are blocking). Resolves the OPF and
/// cover entries via the read-only `epub` crate, then streams the source
/// archive into `dst`, swapping only those two entries.
fn rewrite_blocking(src: &Path, dst: &Path, book: &EbookMetadata) -> anyhow::Result<()> {
    let doc = epub::doc::EpubDoc::new(src).context("open source epub")?;
    let opf_path = zip_name(&doc.root_file);

    // The cover *slot* (manifest id → zip entry path + declared media-type),
    // resolved by the epub crate's EPUB2/EPUB3 cover logic.
    let cover_slot = doc.get_cover_id().and_then(|id| {
        doc.resources
            .get(&id)
            .map(|r| (zip_name(&r.path), r.mime.clone()))
    });

    let new_cover = match (book.has_cover_override, &cover_slot) {
        (true, Some((_, mime))) => book
            .unique_identifier
            .as_deref()
            .and_then(crate::covers::find_override_cover_file)
            .and_then(|(ov_mime, ov_bytes)| encode_cover_for(mime, &ov_mime, ov_bytes)),
        (true, None) => {
            // A user uploaded a cover, but the EPUB embeds none to replace.
            // Adding a fresh cover resource + manifest item is a follow-up;
            // for now the metadata still bakes in.
            tracing::info!(target: "omnibus::epub_rewrite", book_id = book.id, "cover override present but epub has no embedded cover slot; skipping cover swap");
            None
        }
        _ => None,
    };
    let cover_arg = match (cover_slot, new_cover) {
        (Some((path, _)), Some(bytes)) => Some((path, bytes)),
        _ => None,
    };

    rewrite_archive(
        src,
        dst,
        &opf_path,
        |raw| transform_opf(raw, book),
        cover_arg,
    )
}

/// Rewrites every book's export-EPUB cache to bake in its active override, the fleet-wide sibling of the per-book [`rewritten_epub_path`] bake; a per-book failure is recorded rather than aborting the run.
pub async fn rewrite_all_epubs_with_overrides(
    pool: &SqlitePool,
) -> Result<BulkRewriteSummary, EpubRewriteError> {
    // An existing `metadata_overrides` row IS "has an active override" — the
    // write paths that clear overrides entirely (`delete_metadata_overrides`,
    // `clear_cover_override`) delete the row rather than leaving an empty one.
    let uuids: Vec<String> = sqlx::query_scalar("SELECT book_uuid FROM metadata_overrides")
        .fetch_all(pool)
        .await
        .map_err(crate::books::BooksError::Db)?;

    let mut summary = BulkRewriteSummary::default();
    for uuid in uuids {
        let book_id = match crate::books::resolve_book_id_by_uuid(pool, &uuid).await? {
            Some(id) => id,
            // Ghosted: the override row outlived its book (source file
            // removed by a scan, or the book deleted concurrently).
            None => {
                summary.skipped += 1;
                continue;
            }
        };
        let source = match crate::books::book_file_path(pool, book_id, "EPUB").await? {
            Some(path) => path,
            // No EPUB to bake into — an audiobook-only or comic-only book.
            None => {
                summary.skipped += 1;
                continue;
            }
        };
        match rewritten_epub_path(pool, book_id, &source).await {
            Ok(Some(_)) => summary.rewritten += 1,
            // The override cleared between the listing query above and this
            // call (a concurrent delete) — no longer this pass's problem.
            Ok(None) => summary.skipped += 1,
            Err(e) => summary.errors.push(BulkRewriteError {
                book_uuid: uuid,
                message: e.to_string(),
            }),
        }
    }
    Ok(summary)
}

/// A zip entry name from an in-archive path: `to_string_lossy` with backslashes
/// normalized to forward slashes (zip entries are always `/`-separated).
fn zip_name(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

/// `true` when the cached rewritten EPUB is missing or no newer than the book's
/// `last_modified` (so an override edit forces a rebuild). Mirrors
/// `kepub::is_stale`, including the `<=` tie-break for whole-second epochs.
async fn is_stale(path: &Path, last_modified_epoch: i64) -> bool {
    match tokio::fs::metadata(path).await {
        Err(_) => true,
        Ok(meta) => {
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .and_then(|d| i64::try_from(d.as_secs()).ok())
                .unwrap_or(0);
            mtime <= last_modified_epoch
        }
    }
}

#[cfg(test)]
mod tests;
