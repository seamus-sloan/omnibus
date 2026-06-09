//! Unit tests for the `settings` module — `get_settings`/`set_settings`
//! roundtrip, env-var seeding (serialized via a process-global guard),
//! library-prune-on-change cascade, and the batched
//! `prune_orphan_libraries` chunking loop.

use super::*;
use crate::books::list_books;
use crate::covers::{cover_path_for, delete_cover_files_for, write_cover_file};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

// ---------------------------------------------------------------------------
// Process-global env guard
// ---------------------------------------------------------------------------
//
// Serialization + restore guard for the `seed_settings_from_env_*` tests.
// They mutate `EBOOK_LIBRARY_PATH` / `AUDIOBOOK_LIBRARY_PATH`, which is
// process-global; under `cargo test`'s default parallel execution two
// tests racing those vars can observe a torn state. Mirrors
// `test_support::CoversTempDir`: acquiring the guard locks `ENV_LOCK` and
// snapshots both vars, and dropping it restores their prior values (so
// the rest of the test run — and local dev — sees them unchanged) before
// releasing the lock. Keeping the `MutexGuard` in a struct field (rather
// than a bare `let _g`) also keeps it off the await points in the async
// tests.

static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that serializes and restores the library-path env vars.
struct LibraryEnvGuard {
    prev_ebook: Option<String>,
    prev_audiobook: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl LibraryEnvGuard {
    fn acquire() -> Self {
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

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
