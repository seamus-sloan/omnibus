//! Shared test-only helpers for the `omnibus-db` crate.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]` so non-test
//! consumers don't pay the compile cost. This is the single source of
//! truth for cross-cutting helpers — temp-dir builders, in-memory pool
//! seeders, env-var guards, and the `IndexedBook` factories. Module-local
//! ones (e.g. ebook fixture loaders) stay next to their callers.

#![cfg(any(test, feature = "test-support"))]

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use sqlx::SqlitePool;

use crate::ebook::IndexedBook;
use omnibus_shared::{Contributor, EbookMetadata};

// ---------------------------------------------------------------------------
// Filesystem helpers
// ---------------------------------------------------------------------------

/// Build a unique temp directory per test invocation. Rust runs unit
/// tests in parallel by default, so a fixed path under `temp_dir()`
/// would collide between tests (and between repeated runs).
pub fn make_test_dir(suffix: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omnibus_test_{suffix}_{pid}_{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("should create test dir");
    dir
}

// ---------------------------------------------------------------------------
// Generic env-var guard
// ---------------------------------------------------------------------------

/// Process-wide lock serializing all env-var mutations in tests. A single
/// lock is used so guards for different variables still serialize against
/// one another — env-var space is process-global.
pub static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that sets one or more env vars for the lifetime of the
/// test, then restores each previous value on drop. Acquires `ENV_LOCK`
/// so parallel `cargo test` runs can't race on shared variables.
///
/// Create with `EnvVarGuard::set(var, value)`, then chain `.also_set(var,
/// value)` for additional vars — all managed under the single `ENV_LOCK`
/// acquisition. Pass `None` for `value` to _remove_ a variable for the
/// test's duration (the previous value is still restored on drop).
///
/// Use the `_os` variants when the value is an `OsStr` (e.g. a
/// `tempfile::TempDir`'s path) to avoid `to_str().unwrap()` at call sites
/// and to round-trip non-UTF-8 pre-existing values correctly.
pub struct EnvVarGuard {
    /// `(var_name, previous_value)` pairs, restored in reverse on drop.
    /// Snapshots use `OsString` via `var_os` so a pre-existing non-UTF-8
    /// value is restored verbatim instead of being treated as "unset".
    vars: Vec<(&'static str, Option<OsString>)>,
    _lock: std::sync::MutexGuard<'static, ()>,
}

impl EnvVarGuard {
    /// Acquire `ENV_LOCK`, save the current value of `var`, and set it to
    /// `value` (or remove it when `value` is `None`).
    pub fn set(var: &'static str, value: Option<&str>) -> Self {
        Self::set_os(var, value.map(OsStr::new))
    }

    /// `OsStr` variant of [`set`] — accepts paths (e.g.
    /// `tmp.path().as_os_str()`) without forcing the caller through
    /// `to_str().unwrap()`.
    pub fn set_os(var: &'static str, value: Option<&OsStr>) -> Self {
        let lock = ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prev = std::env::var_os(var);
        // SAFETY: ENV_LOCK is held; no other thread in this process
        // mutates the environment concurrently.
        unsafe { Self::apply(var, value) }
        Self {
            vars: vec![(var, prev)],
            _lock: lock,
        }
    }

    /// Set an additional env var under the already-held `ENV_LOCK`. The
    /// previous value is restored (in LIFO order) when the guard drops.
    pub fn also_set(self, var: &'static str, value: Option<&str>) -> Self {
        self.also_set_os(var, value.map(OsStr::new))
    }

    /// `OsStr` variant of [`also_set`].
    pub fn also_set_os(mut self, var: &'static str, value: Option<&OsStr>) -> Self {
        let prev = std::env::var_os(var);
        // SAFETY: ENV_LOCK is still held via `self._lock`.
        unsafe { Self::apply(var, value) }
        self.vars.push((var, prev));
        self
    }

    /// Inner: apply one set/remove under an already-held lock.
    unsafe fn apply(var: &'static str, value: Option<&OsStr>) {
        match value {
            Some(v) => std::env::set_var(var, v),
            None => std::env::remove_var(var),
        }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        // SAFETY: ENV_LOCK is still held via `_lock`; no other thread
        // can mutate the environment while we restore. Restore in reverse
        // insertion order so nested also_set chains unwind correctly.
        for (var, prev) in self.vars.drain(..).rev() {
            unsafe {
                match prev {
                    Some(v) => std::env::set_var(var, v),
                    None => std::env::remove_var(var),
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Covers env guard
// ---------------------------------------------------------------------------

/// Process-wide lock for `OMNIBUS_COVERS_DIR`. Tests that touch the
/// covers directory must serialize, since the env var is global.
///
/// Prefer `EnvVarGuard` + `ENV_LOCK` for new callers. This alias is
/// retained for test code that still references it directly.
pub static COVERS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// RAII guard that points `OMNIBUS_COVERS_DIR` at a unique temp dir for
/// the lifetime of the test, then restores the previous value (and
/// removes the temp dir) on drop. Holds `COVERS_ENV_LOCK` so parallel
/// `cargo test` runs don't stomp on each other.
pub struct CoversTempDir {
    pub path: PathBuf,
    prev: Option<String>,
    _guard: std::sync::MutexGuard<'static, ()>,
}

impl CoversTempDir {
    pub fn new(tag: &str) -> Self {
        let guard = COVERS_ENV_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let pid = std::process::id();
        let seq = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("omnibus_covers_{tag}_{pid}_{seq}"));
        let _ = std::fs::remove_dir_all(&path);
        let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
        // SAFETY: COVERS_ENV_LOCK is held; no other thread mutates the env.
        unsafe {
            std::env::set_var("OMNIBUS_COVERS_DIR", &path);
        }
        Self {
            path,
            prev,
            _guard: guard,
        }
    }
}

impl Drop for CoversTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
        // SAFETY: COVERS_ENV_LOCK is still held via `_guard`.
        unsafe {
            match self.prev.take() {
                Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
                None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal book row seeder (bypasses the indexer)
// ---------------------------------------------------------------------------

/// Seed `count` minimal `books` rows under `/lib` using a recursive CTE.
/// Bypasses `replace_books` / the indexer entirely — callers that only
/// need rows to exist (e.g. response-cap tests) avoid the multi-second
/// cost of running the full pipeline.
pub async fn seed_minimal_books(pool: &SqlitePool, count: i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        r#"
        WITH RECURSIVE n(i) AS (
            SELECT 1
            UNION ALL
            SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, library_id, path, title, sort)
        SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i)
          FROM n
        "#,
    )
    .bind(count)
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
}

// ---------------------------------------------------------------------------
// IndexedBook factories (formerly `sync::test_helpers`)
// ---------------------------------------------------------------------------

/// Shared `IndexedBook` builder used by db tests across modules.
pub fn indexed(
    filename: &str,
    title: Option<&str>,
    authors: &[&str],
    subjects: &[&str],
    series: Option<(&str, &str)>,
    cover: Option<(&str, &[u8])>,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: title.map(Into::into),
            creators: authors
                .iter()
                .map(|a| Contributor {
                    name: (*a).into(),
                    ..Default::default()
                })
                .collect(),
            subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
            series: series.map(|(n, _)| n.into()),
            series_index: series.map(|(_, i)| i.into()),
            ..Default::default()
        },
        cover: cover.map(|(m, b)| (m.into(), b.to_vec())),
        mtime_epoch: 0,
        size_bytes: 0,
    }
}

/// Build an `IndexedBook` matching `indexed(...)` but with the supplied
/// (mtime_epoch, size_bytes). Used to drive the New + Changed branches
/// of `sync_books` with realistic fs metadata.
pub fn indexed_with_stat(
    filename: &str,
    title: Option<&str>,
    mtime_epoch: i64,
    size_bytes: i64,
) -> IndexedBook {
    IndexedBook {
        metadata: EbookMetadata {
            filename: filename.into(),
            title: title.map(Into::into),
            ..Default::default()
        },
        cover: None,
        mtime_epoch,
        size_bytes,
    }
}

// ---------------------------------------------------------------------------
// IndexedAudiobook factories
// ---------------------------------------------------------------------------

/// Shared `IndexedAudiobook` builder for tests that need audiobook rows.
pub fn indexed_audiobook(
    group_path: &str,
    title: &str,
    creator: Option<&str>,
) -> crate::audiobook::IndexedAudiobook {
    crate::audiobook::IndexedAudiobook {
        uuid: format!("ab-uuid-{}", group_path.replace('/', "-")),
        group_path: group_path.into(),
        format: "M4B".into(),
        title: title.into(),
        creator_name: creator.map(Into::into),
        cover: None,
        accent: None,
        parts: vec![crate::audiobook::AudiobookPart {
            ordinal: 0,
            filename: format!("{group_path}/part1.m4b"),
            size_bytes: 1000,
            mtime_epoch: 100,
            duration_seconds: 3600.0,
        }],
        chapters: vec![],
        total_size_bytes: 1000,
        max_mtime_epoch: 100,
        description: Some("Audiobook · 1h 00m".into()),
        error: None,
    }
}

// ---------------------------------------------------------------------------
// Discovery fixture seeders (formerly `discovery::test_helpers`)
// ---------------------------------------------------------------------------

/// Seed a small multi-author, multi-series, multi-tag fixture for the
/// discovery query tests. Returns the pool and a `CoversTempDir` guard
/// the caller must keep alive for the lifetime of the test.
pub async fn seed_discovery_fixture() -> (SqlitePool, CoversTempDir) {
    let guard = CoversTempDir::new("discovery");
    let pool = crate::pool::init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![
            // Two-author book in Saga #1 with tag "fiction"
            indexed(
                "saga1.epub",
                Some("Saga: Book One"),
                &["Ada Lovelace", "Grace Hopper"],
                &["fiction", "classic"],
                Some(("Saga", "1")),
                None,
            ),
            // Sequel in Saga #2, same primary author + new tag
            indexed(
                "saga2.epub",
                Some("Saga: Book Two"),
                &["Ada Lovelace"],
                &["fiction"],
                Some(("Saga", "2")),
                None,
            ),
            // Standalone by Ada — no series
            indexed(
                "standalone.epub",
                Some("Standalone"),
                &["Ada Lovelace"],
                &["essay"],
                None,
                None,
            ),
            // Different-author, different-series book
            indexed(
                "other.epub",
                Some("Other Story"),
                &["Niklaus Wirth"],
                &["nonfiction"],
                Some(("Pioneers", "1")),
                None,
            ),
        ],
    )
    .await
    .unwrap();
    (pool, guard)
}

/// Look up an `authors.id` by name. Panics on miss — test helper only.
pub async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Look up a `series.id` by name. Panics on miss — test helper only.
pub async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// Seed `count` minimal `books` rows under `/lib`, all linked to one
/// author ("Prolific") and one series ("Mega"), via recursive CTEs.
/// Bypasses `replace_books`/the indexer — the discovery read caps only
/// depend on link rows existing — keeping the test fast even past the
/// 1k cap. Returns `(author_id, series_id)`.
pub async fn seed_books_for_one_author_and_series(pool: &SqlitePool, count: i64) -> (i64, i64) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    let author_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Prolific', 'Prolific') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let series_id: i64 =
        sqlx::query_scalar("INSERT INTO series (name, sort) VALUES ('Mega', 'Mega') RETURNING id")
            .fetch_one(pool)
            .await
            .unwrap();
    sqlx::query(
        r#"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO books (uuid, library_id, path, title, sort, series_index)
        SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
               'Title ' || printf('%010d', i), i
          FROM n
        "#,
    )
    .bind(count)
    .bind(lib_id)
    .execute(pool)
    .await
    .unwrap();
    // Link every seeded book to the author and the series.
    sqlx::query(
        "INSERT INTO books_authors_link (book, author, position)
         SELECT id, ?, 0 FROM books",
    )
    .bind(author_id)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books_series_link (book, series)
         SELECT id, ? FROM books",
    )
    .bind(series_id)
    .execute(pool)
    .await
    .unwrap();
    (author_id, series_id)
}

// ---------------------------------------------------------------------------
// Cross-format attach / merge fixture seeders
// ---------------------------------------------------------------------------

/// Index one ebook under `/ebooks` through the real `sync_books` write
/// path and return its stable uuid. Shared by the attach and merge test
/// suites so the seeding shape can't drift between them.
pub async fn seed_synced_ebook(
    pool: &SqlitePool,
    filename: &str,
    title: &str,
    author: &str,
) -> String {
    crate::sync::sync_books(
        pool,
        "/ebooks",
        crate::sync::SyncPlan {
            new_books: vec![indexed(filename, Some(title), &[author], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    crate::helpers::stable_uuid("/ebooks", filename)
}

/// Index one audiobook under `/audio` through the real `sync_audiobooks`
/// write path and return its uuid.
pub async fn seed_synced_audiobook(
    pool: &SqlitePool,
    group_path: &str,
    title: &str,
    author: &str,
) -> String {
    let ab = indexed_audiobook(group_path, title, Some(author));
    let uuid = ab.uuid.clone();
    crate::sync::sync_audiobooks(
        pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    uuid
}

/// One-scalar COUNT helper for table-shape assertions.
pub async fn count_rows(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar(sql).fetch_one(pool).await.unwrap()
}
