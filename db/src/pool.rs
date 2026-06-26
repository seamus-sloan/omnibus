//! SQLite pool initialization for the omnibus data layer. Owns
//! `init_db` (which runs the embedded migrations, applies per-connection
//! PRAGMAs, and performs the one-time legacy cover-cache cleanup).

use std::path::Path;

use sqlx::{sqlite::SqlitePoolOptions, Executor, SqlitePool};

use crate::covers::covers_dir;
use crate::identity::IdentityError;
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
}

/// Initialize or open the SQLite pool at `database_url`, apply per-connection PRAGMAs, run pending
/// migrations, and — on non-memory databases — perform a one-time legacy cover-cache directory purge.
pub async fn init_db(database_url: &str) -> Result<SqlitePool, InitDbError> {
    // PRAGMAs `foreign_keys`, `busy_timeout`, and `synchronous` are
    // *per-connection* settings — they only apply to the connection that
    // executed them, and any future connection the pool spins up would
    // start with SQLite's defaults. Apply them inside `after_connect` so
    // every pooled connection initializes the same way.
    //
    // `journal_mode = WAL` is a database-level setting that lives in the
    // SQLite header, so it only needs to take effect once; running it on
    // every connection is cheap (returns the current mode) and keeps the
    // logic in one place. We still skip it for in-memory databases so the
    // test output isn't littered with "memory" pragma results.
    //
    // See issue #82 for the rationale on each PRAGMA value.
    let is_memory = is_memory_url(database_url);
    let pool = SqlitePoolOptions::new()
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
        .await?;

    // Migration failures surface as `InitDbError::Migrate` via `#[from]`;
    // previously this hand-wrapped `sqlx::migrate::MigrateError` inside
    // `sqlx::Error::Migrate` just so the `?` could ride the
    // `From<sqlx::Error>` impl — pointless indirection that buried the
    // category of failure under a generic `sqlx::Error` envelope.
    MIGRATOR.run(&pool).await?;

    // One-time fill of the auto-attach match key for rows indexed before
    // migration 0016. Idempotent and a no-op once caught up.
    crate::normalize::backfill_norm_columns(&pool).await?;

    // One-time fill of the F2 `scan_key` diff key for rows indexed before
    // migration 0026. Pure DB work (reconstructed from stored columns, no
    // filesystem reads), idempotent, safe against in-memory test DBs.
    crate::identity::backfill_scan_keys(&pool).await?;

    // One-time fill of the F5b `series_sort` keyset column for rows indexed
    // before migration 0028. Reconstructed from the existing series link;
    // idempotent and a no-op once every linked book is filled.
    crate::sort_keys::backfill_series_sort(&pool).await?;

    // Issue #94: the previous `stable_uuid` implementation hashed via
    // `DefaultHasher` and produced toolchain-dependent UUIDs. Switching to
    // UUIDv5 changes every cover id on the next reindex, so any pre-existing
    // `<old-uuid>.<ext>` files in the covers directory would be orphaned and
    // never served again. Purge once on startup, then drop a sentinel so we
    // don't keep deleting freshly-written covers on every boot. Gated on a
    // real (non-memory) DB so that the rapid-fire test suite doesn't touch
    // the developer's actual covers directory.
    if !is_memory {
        // The purge walks the covers dir and unlinks every legacy file with
        // synchronous `std::fs`. On the worst-case first boot after the #94
        // upgrade that directory can be large, so run it on the blocking pool
        // rather than tying up an async runtime worker for the whole sweep.
        // Boot still awaits completion here — this keeps the runtime's other
        // tasks schedulable during the sweep, it does not make startup
        // non-blocking. `JoinError` (a panic in the sweep) is logged and
        // swallowed — the covers dir is a rebuildable cache, so a failed
        // purge must not abort boot.
        let dir = covers_dir();
        if let Err(join_err) = tokio::task::spawn_blocking(move || {
            purge_legacy_covers_once(&dir);
        })
        .await
        {
            tracing::error!("issue #94: legacy cover purge spawn_blocking failed: {join_err}");
        }
    }

    Ok(pool)
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
