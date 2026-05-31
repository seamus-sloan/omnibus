//! Shared test-only helpers for the `omnibus-db` crate.
//!
//! Gated behind `#[cfg(any(test, feature = "test-support"))]` so non-test
//! consumers don't pay the compile cost. This is the single source of
//! truth for cross-cutting helpers — temp-dir builders, in-memory pool
//! seeders, env-var guards, and the `IndexedBook` factories. Module-local
//! ones (e.g. ebook fixture loaders) stay next to their callers.

#![cfg(any(test, feature = "test-support"))]

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
// Covers env guard
// ---------------------------------------------------------------------------

/// Process-wide lock for `OMNIBUS_COVERS_DIR`. Tests that touch the
/// covers directory must serialize, since the env var is global.
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
        std::env::set_var("OMNIBUS_COVERS_DIR", &path);
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
        match self.prev.take() {
            Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
            None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
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
    sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
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
    sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
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
