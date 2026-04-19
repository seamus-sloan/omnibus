//! Background ebook indexing (server-only).
//!
//! The web and mobile list endpoints read from the `books` table instead of
//! walking the filesystem on every request. This module owns the write side:
//! scan the configured library, then atomically replace the DB rows for
//! that path.
//!
//! Two triggers fire a reindex:
//! - On startup, if no index exists yet or the existing one is older than
//!   [`REFRESH_AFTER_SECS`].
//! - On every settings save (the library path may have changed, and even if
//!   it didn't the user likely just added or removed books).
//!
//! Scans run on the blocking pool via `spawn_blocking` so the hot axum
//! runtime stays responsive while the walk + OPF parse + cover reads go.

use sqlx::SqlitePool;

use crate::{db, ebook};

/// Reindex if the last successful index is older than this. One hour is a
/// compromise between responsiveness to on-disk changes and avoiding
/// thrashing the disk for users who leave the app open all day.
pub const REFRESH_AFTER_SECS: i64 = 60 * 60;

/// True when a refresh should be kicked off: no state at all, or state
/// older than [`REFRESH_AFTER_SECS`].
pub async fn is_stale(pool: &SqlitePool, library_path: &str) -> Result<bool, sqlx::Error> {
    let Some(last) = db::last_indexed_at(pool, library_path).await? else {
        return Ok(true);
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(last);
    Ok(now - last >= REFRESH_AFTER_SECS)
}

/// Scan `library_path` and replace the DB index for it. Runs the scan on
/// the blocking pool so callers can `await` it from a normal async context
/// without blocking the runtime.
///
/// A fatal scan error (missing or unreadable root) is returned as `Err` and
/// the existing index is **not** touched — we'd rather serve stale-but-good
/// data than wipe the table and mark the index "fresh" (which would also
/// suppress retries until [`REFRESH_AFTER_SECS`] elapses). Per-book parse
/// failures are *not* fatal; they land in the DB as rows with `error =
/// Some(_)`, same as before.
pub async fn reindex(pool: &SqlitePool, library_path: String) -> anyhow::Result<()> {
    let path_for_scan = library_path.clone();
    let scan = tokio::task::spawn_blocking(move || ebook::scan_ebook_library(Some(&path_for_scan)))
        .await?;
    if let Some(msg) = scan.error {
        anyhow::bail!("scan of {library_path} failed: {msg}");
    }
    db::replace_books(pool, &library_path, scan.books).await?;
    Ok(())
}

/// Spawn a background reindex if stale. Silent on success — the next list
/// request will see the new rows. Errors are logged but not surfaced.
pub fn spawn_reindex_if_stale(pool: SqlitePool, library_path: String) {
    tokio::spawn(async move {
        match is_stale(&pool, &library_path).await {
            Ok(false) => return,
            Ok(true) => {}
            Err(e) => {
                eprintln!("indexer: could not read index state: {e}");
                return;
            }
        }
        if let Err(e) = reindex(&pool, library_path).await {
            eprintln!("indexer: reindex failed: {e}");
        }
    });
}
