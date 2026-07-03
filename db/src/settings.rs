//! Settings KV CRUD, scan-root row upserts, and orphan-scan-root pruning.
//!
//! `settings` KV keys (`ebook_library_path` / `audiobook_library_path`) are
//! read by the UI and translated into `scan_roots` rows by the indexer; saving
//! settings prunes any orphan root and its books, FTS rows, and on-disk covers.

use std::path::Path;

use sqlx::{SqlitePool, Transaction};

pub use omnibus_shared::{HardcoverKeyStatus, Settings, HARDCOVER_API_KEY_MAX_LEN};

/// `settings` KV keys consumed by the UI/RPC layer. Kept as constants so the
/// indexer, settings handlers, and tests all reference the same identifier.
const EBOOK_LIBRARY_PATH_KEY: &str = "ebook_library_path";
const AUDIOBOOK_LIBRARY_PATH_KEY: &str = "audiobook_library_path";
/// `settings` KV key for the F3.3 Hardcover API token. Stored directly (not via
/// the [`Settings`] struct) so saving it never triggers the scan-root
/// reconciliation `set_settings` runs.
const HARDCOVER_API_KEY_KEY: &str = "hardcover_api_key";

/// Errors returned by the settings data layer.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// Caller-supplied value violates a byte-length / shape constraint enforced
    /// before the write. Surfaced as 4xx by handlers so an admin sees a
    /// per-case message rather than a generic 500.
    #[error("{0}")]
    Validation(String),
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
/// `scan_roots` rows (and their books / FTS rows via cascade), then deletes
/// the matching cover files from disk *after* commit so filesystem
/// side-effects don't run inside the DB transaction. Callers should kick
/// off a reindex afterwards — this function does not touch the indexer.
pub async fn set_settings(pool: &SqlitePool, settings: &Settings) -> Result<(), SettingsError> {
    // Read the *current* paths before writing so a changed path can be
    // repointed in place (F2) — keeping the `scan_roots` row's id, and thus
    // every `books.uuid` keyed under it, stable across the move.
    let current = get_settings(pool).await?;
    let mut tx = pool.begin().await?;
    // The other slot's *new* path is passed so a shared scan root (both slots
    // pointing at one path) isn't silently moved when only one slot changes.
    repoint_scan_root(
        &mut tx,
        current.ebook_library_path.as_deref(),
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    )
    .await?;
    repoint_scan_root(
        &mut tx,
        current.audiobook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
        settings.ebook_library_path.as_deref(),
    )
    .await?;
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

/// Repoint a slot's `scan_roots` row in place when its configured path
/// changes (F2). Updating the existing row's `path` (rather than letting
/// `upsert_library` insert a new one) keeps the row id — and therefore every
/// `books.uuid` keyed under that `library_id` — stable, so the next reindex
/// matches each file by its unchanged relative `scan_key` and preserves its
/// identity. The attached-file location overrides (`book_files.library_path`)
/// and the attach ledger (`merged_uuids.library_path`) are repointed in the
/// same step so attachments stay discoverable and resolve to the new root.
///
/// A no-op unless both old and new paths are set and differ; skipped if a row
/// for the new path already exists (avoid a UNIQUE collision — the existing
/// row wins and the reindex reconciles), and skipped when `old` is still
/// referenced by the *other* slot's new path (a shared scan root must not be
/// moved out from under the slot that still uses it).
async fn repoint_scan_root(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    old: Option<&str>,
    new: Option<&str>,
    other_new: Option<&str>,
) -> Result<(), SettingsError> {
    let (Some(old), Some(new)) = (old, new) else {
        return Ok(());
    };
    if old == new {
        return Ok(());
    }
    // Shared-root guard: the other slot still points at `old`, so renaming the
    // row would silently move both libraries and leave the other slot dangling.
    if other_new == Some(old) {
        return Ok(());
    }
    let new_exists: Option<i64> = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(new)
        .fetch_optional(&mut **tx)
        .await?;
    if new_exists.is_some() {
        return Ok(());
    }
    let display_name = Path::new(new)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(new)
        .to_string();
    sqlx::query("UPDATE scan_roots SET path = ?, display_name = ? WHERE path = ?")
        .bind(new)
        .bind(&display_name)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    // Repoint the file-location overrides + attach ledger that key on the
    // *file's* scanned root (set only for cross-format attachments) so the
    // reindex diff (`list_merged_rows_for_formats`) and `book_file_path*`
    // resolve to the new root instead of the stale one.
    sqlx::query("UPDATE merged_uuids SET library_path = ? WHERE library_path = ?")
        .bind(new)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE book_files SET library_path = ? WHERE library_path = ?")
        .bind(new)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Drop **childless** orphan `scan_roots` rows whose `path` is not in `keep`.
///
/// Never-prune (F2): a removed scan root that still owns books is **kept**,
/// and its books (plus their soft-ref user data) are retained rather than
/// cascade-deleted — the durable-identity safety net. Books under a cleared
/// path simply stop being listed (no `list_books` call passes that path) and
/// reappear if it is re-added. Only roots with zero books are swept, to bound
/// empty-row accumulation; nothing user-facing is deleted, so the returned
/// uuid list (for post-commit cover cleanup) is always empty.
pub(crate) async fn prune_orphan_libraries(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    keep: &[Option<&str>],
) -> Result<Vec<String>, SettingsError> {
    let childless_orphans: Vec<i64> = sqlx::query_as("SELECT id, path FROM scan_roots")
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .filter(|(_, path): &(i64, String)| {
            !keep.iter().any(|k| k.map(|s| s == path).unwrap_or(false))
        })
        .map(|(id, _)| id)
        .collect();

    for id in childless_orphans {
        // Conditional delete: the row goes only if it has no books. A root
        // that still owns books is left in place (never-prune) so the FK
        // cascade on `books.library_id` is never triggered.
        sqlx::query(
            "DELETE FROM scan_roots WHERE id = ?
              AND NOT EXISTS (SELECT 1 FROM books WHERE library_id = scan_roots.id)",
        )
        .bind(id)
        .execute(&mut **tx)
        .await?;
    }

    Ok(Vec::new())
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

/// Read the Hardcover API key saved in `settings`, or `None` when unset/blank.
/// This is the raw secret — callers serving it to a client MUST mask it.
pub async fn get_hardcover_api_key(pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
    let v = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(HARDCOVER_API_KEY_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(v.filter(|s| !s.trim().is_empty()))
}

/// Persist (or clear, when `None`/blank) the Hardcover API key in `settings`.
/// Rejects tokens longer than [`HARDCOVER_API_KEY_MAX_LEN`] with
/// [`SettingsError::Validation`] before the write so no admin write path can
/// spill an unbounded blob into the `settings` KV table.
pub async fn set_hardcover_api_key(
    pool: &SqlitePool,
    key: Option<&str>,
) -> Result<(), SettingsError> {
    match key.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) if v.len() > HARDCOVER_API_KEY_MAX_LEN => {
            return Err(SettingsError::Validation(format!(
                "hardcover api key exceeds {HARDCOVER_API_KEY_MAX_LEN} bytes"
            )));
        }
        Some(v) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(HARDCOVER_API_KEY_KEY)
                .bind(v)
                .execute(pool)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(HARDCOVER_API_KEY_KEY)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// The effective Hardcover key: the saved settings value wins; the
/// `HARDCOVER_API_KEY` env var is the fallback when no value is saved. Returns
/// `None` (feature disabled) when neither is set.
pub async fn effective_hardcover_api_key(
    pool: &SqlitePool,
) -> Result<Option<String>, SettingsError> {
    if let Some(saved) = get_hardcover_api_key(pool).await? {
        return Ok(Some(saved));
    }
    Ok(std::env::var("HARDCOVER_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// Masked status of the server-wide Hardcover key: the saved settings value
/// wins (`source = "settings"`), then the `HARDCOVER_API_KEY` env var
/// (`source = "env"`), else unset (`source = "none"`). Never returns the raw
/// key — only a short masked preview. Shared by the REST handler and the RPC
/// server function so the fallback + masking live once.
pub async fn hardcover_key_status(pool: &SqlitePool) -> Result<HardcoverKeyStatus, SettingsError> {
    if let Some(k) = get_hardcover_api_key(pool).await? {
        return Ok(HardcoverKeyStatus {
            configured: true,
            masked: Some(mask_key(&k)),
            source: "settings".to_string(),
        });
    }
    if let Ok(env_key) = std::env::var("HARDCOVER_API_KEY") {
        let env_key = env_key.trim();
        if !env_key.is_empty() {
            return Ok(HardcoverKeyStatus {
                configured: true,
                masked: Some(mask_key(env_key)),
                source: "env".to_string(),
            });
        }
    }
    Ok(HardcoverKeyStatus {
        configured: false,
        masked: None,
        source: "none".to_string(),
    })
}

/// Short masked preview of a secret — never the raw value. Keys of 8 chars or
/// fewer collapse to a fixed bullet run; longer keys (Hardcover tokens are
/// JWTs) render as `first4…last4`.
fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "\u{2022}\u{2022}\u{2022}\u{2022}".to_string();
    }
    let first: String = key.chars().take(4).collect();
    let last: String = key.chars().skip(n - 4).collect();
    format!("{first}\u{2026}{last}")
}

/// Boot hook: seed the Hardcover key from `HARDCOVER_API_KEY` **only** when no
/// value is already saved, so the env var works out of the box without
/// clobbering a key set through Settings on every restart (settings wins).
pub async fn seed_hardcover_key_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    if get_hardcover_api_key(pool).await?.is_some() {
        return Ok(());
    }
    if let Ok(env_key) = std::env::var("HARDCOVER_API_KEY") {
        if !env_key.trim().is_empty() {
            set_hardcover_api_key(pool, Some(&env_key)).await?;
        }
    }
    Ok(())
}

/// Upsert a `scan_roots` row for `path` (display_name derived from basename).
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
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?)
         ON CONFLICT(path) DO UPDATE SET display_name = excluded.display_name",
    )
    .bind(path)
    .bind(&display_name)
    .execute(&mut **tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(path)
        .fetch_one(&mut **tx)
        .await?;
    Ok(id)
}

/// Unix-seconds timestamp of the last successful index for `library_path`,
/// or `None` if the library has never been indexed (or doesn't exist in the
/// `scan_roots` table yet).
pub async fn last_indexed_at(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Option<i64>, SettingsError> {
    Ok(
        sqlx::query_scalar::<_, Option<i64>>("SELECT last_indexed FROM scan_roots WHERE path = ?")
            .bind(library_path)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

#[cfg(test)]
mod tests;
