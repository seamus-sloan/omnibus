//! SQLite pool initialization for the omnibus data layer. Owns
//! `init_db` (which runs the embedded migrations, applies per-connection
//! PRAGMAs, runs boot backfills, and performs legacy cache cleanup).

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
    SeriesNormalize(#[from] crate::series_normalize::SeriesNormalizeError),
    #[error(transparent)]
    MissingFiles(#[from] MissingFilesError),
    #[error(transparent)]
    Shelves(#[from] crate::shelves::ShelfError),
    #[error(transparent)]
    Overrides(#[from] crate::metadata_overrides::MetadataOverridesError),
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
        run_thumb_scheme_purge().await;
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
    // Clear clobbered slots before synthetic ledger rows hide them from reindex.
    repair_ghosted_audiobook_attachments(pool).await?;
    // Auto-attach match key for rows indexed before migration 0016.
    crate::normalize::backfill_norm_columns(pool).await?;
    // Effective (override) match keys for Check-In on rows edited before
    // migration 0048 (and self-heals any drift from the overrides JSON).
    crate::metadata_overrides::backfill_override_norm_columns(pool).await?;
    // F2 `scan_key` diff key for rows indexed before migration 0026,
    // reconstructed from stored columns (also fills `book_files.scan_key`,
    // migration 0043).
    crate::identity::backfill_scan_keys(pool).await?;
    // #1126 recovery: re-plant per-file guards for multi-part audiobook
    // attachments and drop the standalone duplicates the old clobber left.
    // Runs after the scan_key backfill it depends on.
    repair_multipart_audiobook_attachments(pool).await?;
    // #1912: merge `series` rows whose name still embeds a trailing index
    // ("Mistborn #2") into their cleaned canonical row and backfill
    // `books.series_index` for the books that gap-fills. Runs before the
    // series_sort backfill below so that pass reads the merged link.
    crate::series_normalize::backfill_embedded_series_index(pool).await?;
    // F5b `series_sort` keyset column for rows indexed before migration 0028,
    // reconstructed from the existing series link.
    crate::sort_keys::backfill_series_sort(pool).await?;
    // Wishlist/check-in books that gained real files before the attach-time
    // guard existed sit invisible under the Physical pseudo-root — promote them.
    crate::physical::promote_filed_physical_books(pool).await?;
    // F10 missing-files flags for rows already fileless before migration 0029,
    // starting their GC clock at boot time.
    crate::missing_files::backfill_missing_files_flags(pool).await?;
    // #1187 built-in Wishlist shelf for any user missing one (migration 0047
    // seeds existing users; this catches gaps and future rows).
    crate::shelves::provision_wishlist_shelves(pool).await?;
    Ok(())
}

/// Clear audiobook attachment slots bearing the legacy multi-attach clobber
/// signature: multiple ledger rows for one `(book, format)`, backed by exactly
/// one file row. The surviving `books` row and all uuid-keyed user data remain.
async fn repair_ghosted_audiobook_attachments(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let damaged_slots: Vec<(i64, String)> = sqlx::query_as(
        "SELECT mu.book_id, UPPER(mu.format)
           FROM merged_uuids mu
           JOIN book_files bf
             ON bf.book_id = mu.book_id AND bf.format = mu.format
          WHERE UPPER(mu.format) IN ('M4B', 'M4A', 'MP3')
          GROUP BY mu.book_id, UPPER(mu.format)
         HAVING COUNT(*) > 1 AND COUNT(DISTINCT bf.id) = 1",
    )
    .fetch_all(&mut *tx)
    .await?;
    if damaged_slots.is_empty() {
        tx.commit().await?;
        return Ok(());
    }

    for chunk in damaged_slots.chunks(BOOT_REPAIR_CHUNK) {
        let predicate =
            std::iter::repeat_n("(book_id = ? AND format = ? COLLATE NOCASE)", chunk.len())
                .collect::<Vec<_>>()
                .join(" OR ");

        let delete_files_sql = format!("DELETE FROM book_files WHERE {predicate}");
        let mut delete_files = sqlx::query(&delete_files_sql);
        for (book_id, format) in chunk {
            delete_files = delete_files.bind(book_id).bind(format);
        }
        delete_files.execute(&mut *tx).await?;

        let delete_ledger_sql = format!("DELETE FROM merged_uuids WHERE {predicate}");
        let mut delete_ledger = sqlx::query(&delete_ledger_sql);
        for (book_id, format) in chunk {
            delete_ledger = delete_ledger.bind(book_id).bind(format);
        }
        delete_ledger.execute(&mut *tx).await?;
    }

    tx.commit().await?;
    tracing::info!(
        repaired_slots = damaged_slots.len(),
        "boot backfill: cleared ghosted audiobook attachment slots"
    );
    Ok(())
}

/// Repair multi-part audiobook merges damaged before the per-file attach fix
/// (#1126). Two idempotent steps, both `WHERE`-guarded no-ops once healthy:
///
/// 1. Plant a `merged_uuids` guard for every attached audiobook `book_files`
///    row missing one — the state the old `(book, format)` single-occupancy
///    slot left, where all but one part lost its guard and resurrected as a
///    standalone book on each scan. An attachment is identified by its
///    `scan_key` differing from its book's own native `scan_key`.
/// 2. Delete standalone books that duplicate a file now attached elsewhere (the
///    resurrected copies) — but only when they carry **no** user data, so a
///    dupe someone rated/journaled is left for a deliberate manual merge. The
///    `delete_fts` + orphan-taxonomy prune mirror the merge finalize.
///
/// Runs after `backfill_scan_keys` so `book_files.scan_key` is populated.
async fn repair_multipart_audiobook_attachments(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    let orphaned: Vec<(i64, String, String, String)> = sqlx::query_as(
        "SELECT bf.book_id, bf.format, bf.scan_key, COALESCE(bf.library_path, l.path)
           FROM book_files bf
           JOIN books b ON b.id = bf.book_id
           JOIN scan_roots l ON l.id = b.library_id
          WHERE UPPER(bf.format) IN ('M4B', 'M4A', 'MP3')
            AND bf.scan_key IS NOT NULL
            AND bf.scan_key <> COALESCE(b.scan_key, '')
            AND NOT EXISTS (SELECT 1 FROM merged_uuids mu
                             WHERE mu.book_id = bf.book_id AND mu.scan_key = bf.scan_key)",
    )
    .fetch_all(&mut *tx)
    .await?;
    for (book_id, format, scan_key, library_path) in &orphaned {
        // stable_uuid keys the ledger handle to the file's path, matching the
        // attach writer's convention; the reindex matches on scan_key, not this.
        let uuid = crate::helpers::stable_uuid(library_path, scan_key);
        sqlx::query(
            "INSERT OR IGNORE INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(&uuid)
        .bind(book_id)
        .bind(format)
        .bind(library_path)
        .bind(scan_key)
        .execute(&mut *tx)
        .await?;
    }

    // Match the resurrected dupe to the *same physical file* — same scanned
    // root AND relative path — not scan_key alone. `books.scan_key` is only
    // unique per (library_id, scan_key) (0026), so two scan roots sharing a
    // relative path could otherwise delete a healthy book in another library.
    let dupes: Vec<i64> = sqlx::query_scalar(
        "SELECT b.id FROM books b
           JOIN scan_roots lb ON lb.id = b.library_id
          WHERE b.scan_key IS NOT NULL
            AND EXISTS (SELECT 1 FROM book_files bf
                          JOIN books ob        ON ob.id = bf.book_id
                          JOIN scan_roots lo   ON lo.id = ob.library_id
                         WHERE bf.book_id <> b.id
                           AND bf.scan_key = b.scan_key
                           AND COALESCE(bf.library_path, lo.path) = lb.path)
            AND NOT EXISTS (SELECT 1 FROM reading_progress   WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM reading_sessions   WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM listening_sessions WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM bookmarks          WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM user_ratings       WHERE book_uuid = b.uuid)
            AND NOT EXISTS (SELECT 1 FROM journal_entries    WHERE book_uuid = b.uuid)",
    )
    .fetch_all(&mut *tx)
    .await?;
    for id in &dupes {
        crate::sync::delete_fts(&mut tx, *id).await?;
    }
    for chunk in dupes.chunks(BOOT_REPAIR_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("DELETE FROM books WHERE id IN ({placeholders})");
        let mut q = sqlx::query(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        q.execute(&mut *tx).await?;
    }
    if !dupes.is_empty() {
        crate::taxonomy::delete_orphan_taxonomy(&mut tx).await?;
    }

    tx.commit().await?;
    if !orphaned.is_empty() || !dupes.is_empty() {
        tracing::info!(
            planted_guards = orphaned.len(),
            removed_duplicates = dupes.len(),
            "boot backfill: repaired multi-part audiobook attachments (#1126)"
        );
    }
    Ok(())
}

/// Shared chunk size for the boot-repair `IN (...)` batches above: two binds
/// per row in the ghosted-slot repair, one per row in the duplicate-book
/// delete, so 400 keeps either comfortably under SQLite's 999-bind cap.
const BOOT_REPAIR_CHUNK: usize = 400;

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
        tracing::error!(error = %join_err, "legacy cover purge spawn_blocking failed");
    }
}

/// Run the one-time thumbnail-cache purge for the current encoder scheme on
/// the blocking pool.
///
/// Same shape and rationale as [`run_legacy_cover_purge`] above: the sweep is
/// synchronous `std::fs` over a directory that holds three files per book, so
/// it runs on the blocking pool while boot awaits it, and a `JoinError` is
/// logged and swallowed because a rebuildable cache must never abort boot.
/// See `thumbs::purge_stale_scheme_thumbs_once` for why a re-encode needs
/// this at all.
async fn run_thumb_scheme_purge() {
    if let Err(join_err) =
        tokio::task::spawn_blocking(crate::thumbs::purge_stale_scheme_thumbs_once).await
    {
        tracing::error!(error = %join_err, "thumb scheme purge spawn_blocking failed");
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
/// This is deliberately best-effort: an unreadable or uncreatable dir,
/// permission errors, and individual unlink failures are all
/// logged-and-swallowed rather than surfaced. The covers directory is a
/// cache; the worst outcome of failure is a few stale files, not a broken
/// server. The sentinel file ensures we only sweep once per install.
///
/// **The sentinel lands only on a clean sweep.** Writing it after a partial
/// one would mark the directory as current while a legacy file is still
/// sitting in it, orphaned forever. Leaving the sentinel absent costs a
/// re-sweep of an already-empty directory on the next boot.
fn purge_legacy_covers_once(dir: &Path) {
    let sentinel = dir.join(COVERS_SCHEME_SENTINEL);
    if sentinel.exists() {
        return;
    }
    let Some((removed, left_behind)) = sweep_legacy_covers_dir(dir) else {
        return;
    };
    finalize_covers_purge(dir, &sentinel, removed, left_behind);
}

/// One entry's fate in a [`sweep_legacy_covers_dir`] pass.
enum PurgeOutcome {
    Removed,
    LeftBehind,
    Skipped,
}

/// Sweep `dir`'s top-level regular files, removing each one, and return
/// `(removed, left_behind)` counts. `None` means the sweep couldn't
/// proceed at all — an unreadable directory, or a missing one that
/// couldn't be created — and the caller must leave the scheme unmarked.
///
/// A directory that doesn't exist yet has nothing to purge, but still
/// counts as a clean sweep: leaving a fresh install unmarked means its
/// *second* boot finds an unmarked directory and deletes every cover the
/// first boot's indexing run just extracted.
fn sweep_legacy_covers_dir(dir: &Path) -> Option<(usize, usize)> {
    if !dir.exists() {
        return match std::fs::create_dir_all(dir) {
            Ok(()) => Some((0, 0)),
            Err(err) => {
                tracing::warn!(
                    covers_dir = %dir.display(),
                    error = %err,
                    "could not create covers dir to mark the cover scheme; will retry on next boot",
                );
                None
            }
        };
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(
                covers_dir = %dir.display(),
                error = %err,
                "could not read covers dir to purge legacy files; skipping",
            );
            return None;
        }
    };

    let (mut removed, mut left_behind) = (0usize, 0usize);
    for entry in entries {
        match purge_one_legacy_cover(dir, entry) {
            PurgeOutcome::Removed => removed += 1,
            PurgeOutcome::LeftBehind => left_behind += 1,
            PurgeOutcome::Skipped => {}
        }
    }
    Some((removed, left_behind))
}

/// Remove one `read_dir` entry if it's a top-level regular file. Only
/// regular files are touched — subdirectories are left alone, since
/// nothing in this layer writes them today and a future version that does
/// shouldn't have them swept away. An entry that can't be read or stat'd
/// counts as [`PurgeOutcome::LeftBehind`] for the same reason a removal
/// failure does: it can't be verified as clean, so the sentinel must not
/// land.
fn purge_one_legacy_cover(dir: &Path, entry: std::io::Result<std::fs::DirEntry>) -> PurgeOutcome {
    let entry = match entry {
        Ok(e) => e,
        Err(err) => {
            tracing::warn!(
                covers_dir = %dir.display(),
                error = %err,
                "could not read covers dir entry; leaving the scheme unmarked",
            );
            return PurgeOutcome::LeftBehind;
        }
    };
    let path = entry.path();
    match std::fs::metadata(&path) {
        Ok(meta) if !meta.is_file() => return PurgeOutcome::Skipped,
        Ok(_) => {}
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "could not stat covers dir entry; leaving the scheme unmarked",
            );
            return PurgeOutcome::LeftBehind;
        }
    }
    match std::fs::remove_file(&path) {
        Ok(()) => PurgeOutcome::Removed,
        Err(err) => {
            tracing::warn!(
                path = %path.display(),
                error = %err,
                "failed to remove legacy cover file",
            );
            PurgeOutcome::LeftBehind
        }
    }
}

/// Write (or withhold) the covers-scheme sentinel once the sweep is done.
/// **The sentinel lands only on a clean sweep** — writing it after a
/// partial one would mark the directory as current while a legacy file is
/// still sitting in it, orphaned forever.
fn finalize_covers_purge(dir: &Path, sentinel: &Path, removed: usize, left_behind: usize) {
    if left_behind > 0 {
        tracing::warn!(
            covers_dir = %dir.display(),
            removed,
            left_behind,
            "legacy cover purge incomplete; leaving the scheme unmarked so it retries on next boot",
        );
    } else if let Err(err) = std::fs::write(sentinel, b"v5\n") {
        tracing::warn!(
            sentinel = %sentinel.display(),
            error = %err,
            "failed to write covers-scheme sentinel; will retry on next boot",
        );
    } else {
        tracing::info!(
            covers_dir = %dir.display(),
            removed,
            "purged legacy cover files and wrote v5 sentinel",
        );
    }
}

#[cfg(test)]
mod tests;
