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

/// Errors returned by `get_settings`, `set_settings`, and
/// `seed_settings_from_env`. Other public functions in this module still
/// return `sqlx::Error` directly — widening that is tracked separately.
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
) -> Result<Vec<String>, sqlx::Error> {
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
) -> Result<i64, sqlx::Error> {
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
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT last_indexed FROM libraries WHERE path = ?")
        .bind(library_path)
        .fetch_optional(pool)
        .await
        .map(|opt| opt.flatten())
}

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Serialization + restore guard for the `seed_settings_from_env_*`
    //! tests. They mutate `EBOOK_LIBRARY_PATH` / `AUDIOBOOK_LIBRARY_PATH`,
    //! which is process-global; under `cargo test`'s default parallel
    //! execution two tests racing those vars can observe a torn state.
    //! Mirrors `test_support::CoversTempDir`: acquiring the guard
    //! locks `ENV_LOCK` and snapshots both vars, and dropping it restores
    //! their prior values (so the rest of the test run — and local dev — sees
    //! them unchanged) before releasing the lock. Keeping the `MutexGuard` in
    //! a struct field (rather than a bare `let _g`) also keeps it off the
    //! await points in the async tests.

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that serializes and restores the library-path env vars.
    pub(crate) struct LibraryEnvGuard {
        prev_ebook: Option<String>,
        prev_audiobook: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl LibraryEnvGuard {
        pub(crate) fn acquire() -> Self {
            let guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
            Self {
                prev_ebook: std::env::var("EBOOK_LIBRARY_PATH").ok(),
                prev_audiobook: std::env::var("AUDIOBOOK_LIBRARY_PATH").ok(),
                _guard: guard,
            }
        }
    }

    impl Drop for LibraryEnvGuard {
        fn drop(&mut self) {
            for (key, prev) in [
                ("EBOOK_LIBRARY_PATH", self.prev_ebook.take()),
                ("AUDIOBOOK_LIBRARY_PATH", self.prev_audiobook.take()),
            ] {
                match prev {
                    Some(v) => std::env::set_var(key, v),
                    None => std::env::remove_var(key),
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::init_db;
    use crate::settings::test_helpers::LibraryEnvGuard;

    #[tokio::test]
    async fn get_settings_returns_none_for_empty_db() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let settings = get_settings(&pool).await.unwrap();
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn set_and_get_settings_roundtrips() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let input = Settings {
            ebook_library_path: Some("/books/ebooks".into()),
            audiobook_library_path: Some("/books/audio".into()),
        };
        set_settings(&pool, &input).await.unwrap();
        assert_eq!(get_settings(&pool).await.unwrap(), input);
    }

    #[tokio::test]
    async fn set_settings_updates_existing_values() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, Some("/new".into()));
        assert_eq!(result.audiobook_library_path, Some("/audio".into()));
    }

    #[tokio::test]
    async fn set_settings_none_clears_existing_value() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn seed_settings_from_env_writes_env_vars_to_db() {
        // The guard serializes the process-global env mutation against the
        // other `seed_settings_from_env` test and restores prior values on
        // drop, so this test can't leak `EBOOK_LIBRARY_PATH` into the rest of
        // the run.
        let _env = LibraryEnvGuard::acquire();
        let pool = init_db("sqlite::memory:").await.unwrap();
        std::env::set_var("EBOOK_LIBRARY_PATH", "/env/books");
        std::env::set_var("AUDIOBOOK_LIBRARY_PATH", "/env/audio");
        seed_settings_from_env(&pool).await.unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, Some("/env/books".into()));
        assert_eq!(result.audiobook_library_path, Some("/env/audio".into()));
    }

    #[tokio::test]
    async fn seed_settings_from_env_is_noop_when_vars_unset() {
        // Guard restores prior values on drop. We still clear the vars here to
        // establish the "unset" precondition this test exercises.
        let _env = LibraryEnvGuard::acquire();
        let pool = init_db("sqlite::memory:").await.unwrap();
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        seed_settings_from_env(&pool).await.unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    use crate::books::list_books;
    use crate::covers::{cover_path_for, delete_cover_files_for, write_cover_file};
    use crate::sync::replace_books;
    use crate::test_support::{indexed, CoversTempDir};

    #[tokio::test]
    async fn set_settings_prunes_library_when_ebook_path_changes() {
        let _covers = CoversTempDir::new("prune-change");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/old",
            vec![indexed(
                "a.epub",
                Some("Dracula"),
                &["Stoker"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        assert_eq!(list_books(&pool, "/old").await.unwrap().len(), 1);

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        assert!(list_books(&pool, "/old").await.unwrap().is_empty());
        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fts_count, 0);
    }
    #[tokio::test]
    async fn set_settings_keeps_libraries_still_configured() {
        let _covers = CoversTempDir::new("prune-keep");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
    }
    #[tokio::test]
    async fn set_settings_none_removes_library_data() {
        let _covers = CoversTempDir::new("prune-clear");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
    }
    /// Exercises the batched (#149) `prune_orphan_libraries` path across more
    /// than one chunk. The IN-list is chunked at 500 ids to stay under
    /// SQLite's bind-parameter cap, so seeding more orphaned libraries than a
    /// single chunk holds is what actually verifies the chunking loop iterates
    /// (a regression that dropped or mis-bound a later chunk would otherwise go
    /// undetected). Seeds 1001 libraries — three chunks of 500 / 500 / 1 — each
    /// with a book, then prunes them all in one transaction and asserts every
    /// cover UUID is collected (so the lookup is verified to span all chunks)
    /// and every row is gone. Only a handful of rows get an on-disk cover:
    /// materializing 1001 files would be slow, and cover deletion is exercised
    /// by the tracked subset, which straddles the chunk boundaries.
    #[tokio::test]
    async fn prune_orphan_libraries_batches_across_many_libraries() {
        // > 2 full chunks of 500 → the chunk loop runs three times.
        const LIBRARY_COUNT: usize = 1001;
        // Indices spanning every chunk: first row, the first row of the second
        // chunk, a mid-chunk row, and the final (third-chunk) row.
        const MATERIALIZED_COVER_INDICES: [usize; 4] = [0, 500, 750, LIBRARY_COUNT - 1];

        let _covers = CoversTempDir::new("prune-batch");
        let pool = init_db("sqlite::memory:").await.unwrap();

        // Seed the orphaned libraries directly. `keep = []` below marks all of
        // them as orphans regardless of path.
        let mut expected_uuids: Vec<String> = Vec::with_capacity(LIBRARY_COUNT);
        let mut materialized_uuids: Vec<String> = Vec::new();
        for i in 0..LIBRARY_COUNT {
            let path = format!("/orphan-{i}");
            let library_id: i64 = sqlx::query_scalar(
                "INSERT INTO libraries (path, display_name) VALUES (?, ?) RETURNING id",
            )
            .bind(&path)
            .bind(format!("Orphan {i}"))
            .fetch_one(&pool)
            .await
            .unwrap();

            let uuid = format!("uuid-{i}");
            sqlx::query(
                "INSERT INTO books (uuid, library_id, path, title, has_cover)
                 VALUES (?, ?, ?, ?, 1)",
            )
            .bind(&uuid)
            .bind(library_id)
            .bind(format!("{path}/book.epub"))
            .bind(format!("Book {i}"))
            .execute(&pool)
            .await
            .unwrap();

            // Materialize an on-disk cover for a few rows spanning the chunk
            // boundaries so deletion is observable without writing 1001 files.
            if MATERIALIZED_COVER_INDICES.contains(&i) {
                write_cover_file(&uuid, "image/jpeg", b"fake-jpeg").unwrap();
                assert!(cover_path_for(&uuid, "jpg").exists());
                materialized_uuids.push(uuid.clone());
            }

            expected_uuids.push(uuid);
        }

        let mut tx = pool.begin().await.unwrap();
        let mut orphan_uuids = prune_orphan_libraries(&mut tx, &[]).await.unwrap();
        tx.commit().await.unwrap();

        orphan_uuids.sort();
        expected_uuids.sort();
        assert_eq!(
            orphan_uuids, expected_uuids,
            "every orphaned book's cover UUID should be collected across all chunks"
        );

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);

        // The caller deletes cover files post-commit; verify the collected
        // UUIDs drive removal of every materialized cover (one per chunk).
        delete_cover_files_for(&orphan_uuids);
        for uuid in &materialized_uuids {
            assert!(
                !cover_path_for(uuid, "jpg").exists(),
                "cover for {uuid} should be deleted"
            );
        }
    }
}
