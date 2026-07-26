//! Bake a book's effective metadata + cover override *into* a copy of its
//! EPUB (F5.8 #1372), so exports (Send-to-Kobo KEPUB, plain download) carry the
//! user's edits instead of shipping the untouched source file. Unlike the OPF
//! sidecar export (`opf_export`), this writes inside the `.epub` container,
//! which is the only metadata a Kobo / kepubify actually reads.
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
use std::time::UNIX_EPOCH;

use anyhow::Context;
use sqlx::SqlitePool;

use omnibus_shared::EbookMetadata;

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
fn export_epub_path(book_id: i64) -> PathBuf {
    export_epub_dir().join(format!("{book_id}.epub"))
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
    // (an admin who ranks embedded tags above overrides gets no rewrite — the
    // same deliberate gating `opf_export` documents). Nothing to bake → source.
    if !book.has_override && !book.has_cover_override {
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
