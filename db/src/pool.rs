//! SQLite pool initialization for the omnibus data layer. Owns
//! `init_db` (which runs the embedded migrations, applies per-connection
//! PRAGMAs, and performs the one-time legacy cover-cache cleanup).

use std::path::Path;

use sqlx::{sqlite::SqlitePoolOptions, Executor, SqlitePool};

use crate::covers::covers_dir;
use crate::identity::IdentityError;
use crate::missing_files::MissingFilesError;
use crate::normalize::NormalizeError;

/// Schema migrations embedded at compile time from `db/migrations/`.
/// Every schema change ships as a new numbered `.sql` file there; applied
/// versions are recorded in the `_sqlx_migrations` table.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

/// Errors returned by [`init_db`].
#[derive(Debug, thiserror::Error)]
pub enum InitDbError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("database migrations failed: {0}")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    #[error(transparent)]
    Normalize(#[from] NormalizeError),
    #[error(transparent)]
    Identity(#[from] IdentityError),
    #[error(transparent)]
    SortKeys(#[from] crate::sort_keys::SortKeysError),
    #[error(transparent)]
    MissingFiles(#[from] MissingFilesError),
}

/// Initialize or open the SQLite pool at `database_url`, apply per-connection PRAGMAs, run pending
/// migrations, and — on non-memory databases — perform a one-time legacy cover-cache directory purge.
pub async fn init_db(database_url: &str) -> Result<SqlitePool, InitDbError> {
    let is_memory = is_memory_url(database_url);
    let pool = connect_pool(database_url, is_memory).await?;

    MIGRATOR.run(&pool).await?;
    run_boot_backfills(&pool).await?;

    if !is_memory {
        run_legacy_cover_purge().await;
    }

    Ok(pool)
}

/// Build the SQLite pool and register the per-connection PRAGMA setup.
///
/// PRAGMAs `foreign_keys`, `busy_timeout`, and `synchronous` are
/// *per-connection* settings — they only apply to the connection that
/// executed them, and any future connection the pool spins up would start
/// with SQLite's defaults. Applying them inside `after_connect` makes every
/// pooled connection initialize the same way. `journal_mode = WAL` is a
/// database-level setting stored in the SQLite header, so it only needs to
/// take effect once; running it on every connection is cheap (returns the
/// current mode) and keeps the logic in one place. It's skipped for
/// in-memory databases so test output isn't littered with pragma results.
async fn connect_pool(database_url: &str, is_memory: bool) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(5)
        .after_connect(move |conn, _meta| {
            Box::pin(async move {
                conn.execute("PRAGMA foreign_keys = ON").await?;
                conn.execute("PRAGMA busy_timeout = 5000").await?;
                conn.execute("PRAGMA synchronous = NORMAL").await?;
                if !is_memory {
                    conn.execute("PRAGMA journal_mode = WAL").await?;
                }
                Ok(())
            })
        })
        .connect(database_url)
        .await
}

/// Run the one-time, idempotent column backfills that fill values older
/// migrations couldn't compute. Each is a no-op once caught up and is safe
/// against in-memory test DBs (all pure DB work, no filesystem reads).
async fn run_boot_backfills(pool: &SqlitePool) -> Result<(), InitDbError> {
    // Auto-attach match key for rows indexed before migration 0016.
    crate::normalize::backfill_norm_columns(pool).await?;
    // F2 `scan_key` diff key for rows indexed before migration 0026,
    // reconstructed from stored columns.
    crate::identity::backfill_scan_keys(pool).await?;
    // F5b `series_sort` keyset column for rows indexed before migration 0028,
    // reconstructed from the existing series link.
    crate::sort_keys::backfill_series_sort(pool).await?;
    // F10 missing-files flags for rows already fileless before migration 0029,
    // starting their GC clock at boot time.
    crate::missing_files::backfill_missing_files_flags(pool).await?;
    Ok(())
}

/// Run the one-time legacy cover-cache purge on the blocking pool.
///
/// The previous `stable_uuid` implementation hashed via `DefaultHasher` and
/// produced toolchain-dependent UUIDs. Switching to UUIDv5 changes every
/// cover id on the next reindex, so any pre-existing `<old-uuid>.<ext>` files
/// would be orphaned and never served again. `purge_legacy_covers_once`
/// walks the covers dir with synchronous `std::fs`, which can be large on the
/// first boot after the upgrade, so it runs on the blocking pool — boot still
/// awaits completion, keeping other runtime tasks schedulable during the
/// sweep. A `JoinError` (a panic in the sweep) is logged and swallowed: the
/// covers dir is a rebuildable cache, so a failed purge must not abort boot.
async fn run_legacy_cover_purge() {
    let dir = covers_dir();
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        purge_legacy_covers_once(&dir);
    })
    .await
    {
        tracing::error!("issue #94: legacy cover purge spawn_blocking failed: {join_err}");
    }
}

/// Returns `true` if `database_url` points at an in-memory SQLite database.
/// WAL mode requires a real file on disk; sqlx still accepts the PRAGMA
/// against `:memory:` but it's a no-op there.
fn is_memory_url(database_url: &str) -> bool {
    database_url.contains(":memory:") || database_url.contains("mode=memory")
}

/// Sentinel filename written into `covers_dir()` after a one-time cleanup of
/// legacy `DefaultHasher`-derived cover files. Presence of this file marks
/// the directory as already on the UUIDv5 scheme and short-circuits the
/// purge on subsequent boots. See `purge_legacy_covers_once`.
const COVERS_SCHEME_SENTINEL: &str = ".omnibus-cover-scheme-v5";

/// One-time cleanup for the cover cache when upgrading from the legacy
/// `DefaultHasher`-derived UUIDs to UUIDv5. The old derivation produced
/// toolchain-dependent ids, whose output
/// changes between Rust toolchains; the new derivation uses RFC 4122 UUIDv5,
/// which produces different ids for the same `(library_path, filename)`
/// inputs. Any cover files written under the old scheme are now unreachable
/// — the `books.uuid` column will be rewritten on the next reindex, but the
/// orphan files would sit in the covers dir forever.
///
/// This is deliberately best-effort: missing dir, permission errors, and
/// individual unlink failures are all logged-and-swallowed rather than
/// surfaced. The covers directory is a cache; the worst outcome of failure
/// is a few stale files, not a broken server. The sentinel file ensures
/// we only sweep once per install.
fn purge_legacy_covers_once(dir: &Path) {
    // No covers dir yet → nothing to clean, and creating it eagerly would
    // be surprising. The next cover write will create it.
    if !dir.exists() {
        return;
    }
    let sentinel = dir.join(COVERS_SCHEME_SENTINEL);
    if sentinel.exists() {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(
                covers_dir = %dir.display(),
                error = %err,
                "issue #94: could not read covers dir to purge legacy files; skipping",
            );
            return;
        }
    };

    let mut removed: usize = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        // Only touch regular files at the top level. Leave subdirectories
        // alone — nothing in this layer writes them today, but if a future
        // version does we'd rather not blow them away.
        if !path.is_file() {
            continue;
        }
        if let Err(err) = std::fs::remove_file(&path) {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "issue #94: failed to remove legacy cover file",
            );
        } else {
            removed += 1;
        }
    }

    if let Err(err) = std::fs::write(&sentinel, b"v5\n") {
        tracing::warn!(
            sentinel = %sentinel.display(),
            error = %err,
            "issue #94: failed to write covers-scheme sentinel; will retry on next boot",
        );
    } else {
        tracing::info!(
            covers_dir = %dir.display(),
            removed,
            "issue #94: purged legacy cover files and wrote v5 sentinel",
        );
    }
}

#[cfg(test)]
mod tests;
