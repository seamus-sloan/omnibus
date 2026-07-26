//! `convert_book` — idempotent EPUB→KEPUB conversion with an atomic cache
//! write. Serialized per book by the worker's `kepub:{book_id}` resource key,
//! so a burst of first-time downloads collapses onto one kepubify run.

use std::path::{Path, PathBuf};

use anyhow::Context;
use sqlx::SqlitePool;

use super::detect::kepubify_bin;
use super::fs::{is_stale_async, kepub_dir, kepub_path};
use super::KepubError;

/// Convert `book_id`'s EPUB to a cached KEPUB and return its path.
///
/// Returns the cached path unchanged when it is already fresh vs.
/// `books.last_modified` (idempotent). Otherwise runs kepubify into a temp
/// file and atomically renames it into place so a concurrent reader never
/// sees a torn file. Errors when the book/EPUB is missing or kepubify fails —
/// the caller then falls back to serving the plain EPUB.
pub async fn convert_book(pool: &SqlitePool, book_id: i64) -> Result<PathBuf, KepubError> {
    let last_modified = crate::get_last_modified_epoch(pool, book_id)
        .await
        .context("look up book last_modified")?
        .ok_or(KepubError::BookNotFound(book_id))?;

    let out_path = kepub_path(book_id);
    if !is_stale_async(book_id, last_modified).await {
        tracing::debug!(target: "omnibus::kepub", book_id, "kepub cache hit");
        return Ok(out_path);
    }

    let src = crate::book_file_path(pool, book_id, "EPUB")
        .await
        .context("look up book's EPUB source path")?
        .ok_or(KepubError::SourceMissing(book_id))?;

    // Bake DB metadata/cover overrides into a rewritten EPUB first, so the
    // KEPUB the Kobo renders carries the user's edits (F5.8 #1372). No active
    // override → `None`, and we kepubify the source verbatim. A rewrite failure
    // is non-fatal: log and fall back to the source rather than blocking the
    // download.
    let input = match crate::rewritten_epub_path(pool, book_id, &src).await {
        Ok(Some(rewritten)) => rewritten,
        Ok(None) => src.clone(),
        Err(e) => {
            tracing::warn!(target: "omnibus::kepub", book_id, error = %e, "epub override-rewrite failed; kepubifying source");
            src.clone()
        }
    };

    let dir = kepub_dir();
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("create kepub cache dir {}", dir.display()))?;
    // Hidden, per-book temp so a crashed run leaves no half-written cache and
    // different books never collide. `.kepub.epub` suffix keeps kepubify happy.
    let tmp = dir.join(format!(".{book_id}.tmp.kepub.epub"));
    let _ = tokio::fs::remove_file(&tmp).await;

    tracing::info!(target: "omnibus::kepub", book_id, ?input, "converting epub to kepub");
    run_kepubify(&input, &tmp).await?;
    tokio::fs::rename(&tmp, &out_path)
        .await
        .context("move converted kepub into cache")?;
    tracing::info!(target: "omnibus::kepub", book_id, "kepub conversion complete");

    Ok(out_path)
}

/// Run `kepubify -o <out> -- <src>` and bail on a non-zero exit, carrying the
/// captured stderr. The `--` terminates flag parsing so a source path
/// beginning with `-` is always treated as the positional input, never a flag.
async fn run_kepubify(src: &Path, out: &Path) -> anyhow::Result<()> {
    let bin = kepubify_bin();
    tracing::debug!(target: "omnibus::kepub", %bin, ?src, ?out, "spawning kepubify");
    let output = tokio::process::Command::new(&bin)
        .arg("-o")
        .arg(out)
        .arg("--")
        .arg(src)
        .output()
        .await
        .with_context(|| format!("spawn kepubify ({bin})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        anyhow::bail!("kepubify exited with {}: {stderr}", output.status);
    }
    Ok(())
}
