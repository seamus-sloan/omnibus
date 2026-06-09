//! Settings KV CRUD, library row upserts, and orphan-library pruning.
//!
//! The `settings` KV keys (`ebook_library_path` / `audiobook_library_path`)
//! are read by the UI and translated into `libraries` rows by the indexer.
//! Saving settings prunes any library whose path is no longer configured
//! along with its books, FTS rows, and on-disk covers.

use std::path::Path;

use sqlx::{SqlitePool, Transaction};

pub use omnibus_shared::Settings;

/// `settings` KV keys consumed by the UI/RPC layer. Kept as constants so the
/// indexer, settings handlers, and tests all reference the same identifier.
const EBOOK_LIBRARY_PATH_KEY: &str = "ebook_library_path";
const AUDIOBOOK_LIBRARY_PATH_KEY: &str = "audiobook_library_path";

/// Errors returned by the settings data layer.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Read the ebook and audiobook library paths from the `settings` KV table.
/// Missing keys map to `None` rather than an error — the first-run UI relies
/// on this to detect an unconfigured server.
pub async fn get_settings(pool: &SqlitePool) -> Result<Settings, SettingsError> {
    let ebook_library_path =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(EBOOK_LIBRARY_PATH_KEY)
            .fetch_optional(pool)
            .await?;
    let audiobook_library_path =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(AUDIOBOOK_LIBRARY_PATH_KEY)
            .fetch_optional(pool)
            .await?;
    Ok(Settings {
        ebook_library_path,
        audiobook_library_path,
    })
}

/// Persist library paths and reconcile dependent state in a single
/// transaction: upserts each path into `settings`, prunes orphaned
/// `libraries` rows (and their books / FTS rows via cascade), then deletes
/// the matching cover files from disk *after* commit so filesystem
/// side-effects don't run inside the DB transaction. Callers should kick
/// off a reindex afterwards — this function does not touch the indexer.
pub async fn set_settings(pool: &SqlitePool, settings: &Settings) -> Result<(), SettingsError> {
    let mut tx = pool.begin().await?;
    upsert_or_clear(
        &mut tx,
        EBOOK_LIBRARY_PATH_KEY,
        settings.ebook_library_path.as_deref(),
    )
    .await?;
    upsert_or_clear(
        &mut tx,
        AUDIOBOOK_LIBRARY_PATH_KEY,
        settings.audiobook_library_path.as_deref(),
    )
    .await?;
    let orphan_uuids = prune_orphan_libraries(
        &mut tx,
        &[
            settings.ebook_library_path.as_deref(),
            settings.audiobook_library_path.as_deref(),
        ],
    )
    .await?;
    tx.commit().await?;

    // Unlinking the orphaned cover files is synchronous `std::fs` that scales
    // with the orphan count, so move it off the runtime. A `JoinError` (panic
    // in the unlink loop) is logged and swallowed — covers are a rebuildable
    // cache, so a failed cleanup must not fail the settings save.
    if let Err(join_err) =
        tokio::task::spawn_blocking(move || crate::covers::delete_cover_files_for(&orphan_uuids))
            .await
    {
        tracing::error!("set_settings: cover cleanup spawn_blocking failed: {join_err}");
    }
    Ok(())
}

/// Delete every `libraries` row whose `path` is not in `keep`, along with
/// its books, dependent rows removed by book-level cascades, FTS rows, and
/// on-disk cover files. Settings has at most one ebook and one audiobook
/// path, so any library whose path isn't one of those is orphaned and must
/// go — otherwise switching the configured path leaves the old library's
/// rows behind and `list_books` callers for the old path continue to see
/// its data.
///
/// Returns the orphaned books' UUIDs so the caller can delete the matching
/// cover files *after* committing the transaction — filesystem side-effects
/// must not run inside the DB transaction.
pub(crate) async fn prune_orphan_libraries(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    keep: &[Option<&str>],
) -> Result<Vec<String>, SettingsError> {
    let orphans: Vec<(i64, String)> = sqlx::query_as("SELECT id, path FROM libraries")
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .filter(|(_, path): &(i64, String)| {
            !keep.iter().any(|k| k.map(|s| s == path).unwrap_or(false))
        })
        .collect();

    if orphans.is_empty() {
        return Ok(Vec::new());
    }

    let orphan_ids: Vec<i64> = orphans.iter().map(|(id, _)| *id).collect();

    // Collect every orphaned book's cover UUID in a single `IN (...)` query
    // instead of one `SELECT` per library — the cleanup branch runs on every
    // reindex (via `set_settings`/`replace_books`), so the per-library
    // round-trip was an N+1 that degrades linearly with the number of
    // accumulated orphans (#149). We chunk on SQLite's 999-parameter bind
    // limit, matching the `load_overrides_bulk` batching introduced in #77.
    let mut orphan_uuids: Vec<String> = Vec::new();
    for chunk in orphan_ids.chunks(500) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");

        let select_sql = format!("SELECT uuid FROM books WHERE library_id IN ({placeholders})");
        let mut select = sqlx::query_scalar::<_, String>(&select_sql);
        for id in chunk {
            select = select.bind(id);
        }
        let mut uuids = select.fetch_all(&mut **tx).await?;
        orphan_uuids.append(&mut uuids);

        let fts_sql = format!(
            "DELETE FROM books_fts WHERE rowid IN
                (SELECT id FROM books WHERE library_id IN ({placeholders}))"
        );
        let mut fts_delete = sqlx::query(&fts_sql);
        for id in chunk {
            fts_delete = fts_delete.bind(id);
        }
        fts_delete.execute(&mut **tx).await?;

        let books_sql = format!("DELETE FROM books WHERE library_id IN ({placeholders})");
        let mut books_delete = sqlx::query(&books_sql);
        for id in chunk {
            books_delete = books_delete.bind(id);
        }
        books_delete.execute(&mut **tx).await?;

        let libraries_sql = format!("DELETE FROM libraries WHERE id IN ({placeholders})");
        let mut libraries_delete = sqlx::query(&libraries_sql);
        for id in chunk {
            libraries_delete = libraries_delete.bind(id);
        }
        libraries_delete.execute(&mut **tx).await?;
    }

    Ok(orphan_uuids)
}

async fn upsert_or_clear(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &str,
    value: Option<&str>,
) -> Result<(), sqlx::Error> {
    match value {
        Some(v) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(v)
                .execute(&mut **tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(key)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// Boot-time hook that seeds [`Settings`] from `EBOOK_LIBRARY_PATH` /
/// `AUDIOBOOK_LIBRARY_PATH` if either is present. No-op when both are
/// unset, so production deployments that configure libraries through the
/// UI are unaffected. Delegates writes through [`set_settings`], so
/// orphan-cleanup runs the same way as a user-initiated save.
pub async fn seed_settings_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    let ebook_library_path = std::env::var("EBOOK_LIBRARY_PATH").ok();
    let audiobook_library_path = std::env::var("AUDIOBOOK_LIBRARY_PATH").ok();
    if ebook_library_path.is_some() || audiobook_library_path.is_some() {
        set_settings(
            pool,
            &Settings {
                ebook_library_path,
                audiobook_library_path,
            },
        )
        .await?;
    }
    Ok(())
}

/// Upsert a `libraries` row for `path` (display_name derived from basename).
/// Used by the indexer write path in `sync`.
pub(crate) async fn upsert_library(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    path: &str,
) -> Result<i64, SettingsError> {
    let display_name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string();
    sqlx::query(
        "INSERT INTO libraries (path, display_name) VALUES (?, ?)
         ON CONFLICT(path) DO UPDATE SET display_name = excluded.display_name",
    )
    .bind(path)
    .bind(&display_name)
    .execute(&mut **tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = ?")
        .bind(path)
        .fetch_one(&mut **tx)
        .await?;
    Ok(id)
}

/// Unix-seconds timestamp of the last successful index for `library_path`,
/// or `None` if the library has never been indexed (or doesn't exist in the
/// `libraries` table yet).
pub async fn last_indexed_at(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Option<i64>, SettingsError> {
    Ok(
        sqlx::query_scalar::<_, Option<i64>>("SELECT last_indexed FROM libraries WHERE path = ?")
            .bind(library_path)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

#[cfg(test)]
mod tests;
