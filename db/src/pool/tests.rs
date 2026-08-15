//! Tests for pool init and migrations: `init_db` error surfacing on a bad
//! URL or a tampered applied-migration checksum, and per-migration
//! correctness checks (schema cleanup, timestamp coercion, orphan
//! row handling, derived-key reset) run against a fresh `sqlite::memory:`
//! database.

use super::*;

use crate::test_support::{count_rows as count, CoversTempDir};

#[tokio::test]
async fn init_db_returns_db_error_when_url_is_invalid() {
    // `sqlite://` URL pointing at a path under a non-existent dir + no
    // `mode=rwc` flag forces the underlying pool connect to fail. The
    // typed wrapper must surface a `Db` variant rather than panicking
    // or leaking `sqlx::Error` at the signature.
    let err = init_db("sqlite:///nonexistent/dir/omnibus.db")
        .await
        .expect_err("invalid url should fail to open");
    assert!(matches!(err, InitDbError::Db(_)));
}

#[tokio::test]
async fn init_db_returns_migrate_error_when_applied_checksum_is_tampered() {
    // sqlx checksums every migration and, at startup, compares each embedded
    // migration against the checksum recorded in `_sqlx_migrations`. A row
    // whose stored checksum no longer matches (an edited-after-apply migration,
    // per rule 06) must fail startup — the typed wrapper surfaces it as
    // `InitDbError::Migrate`, not a panic or a leaked `MigrateError`.
    //
    // First `init_db` lets sqlx create and populate `_sqlx_migrations` itself
    // (no hand-recreation of its internal schema, which would rot across sqlx
    // versions); then we tamper version 1's checksum in place and re-open. The
    // tempdir auto-removes the DB plus its WAL/`-shm` sidecars on drop.
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());

    let pool = init_db(&url).await.expect("initial init_db should succeed");
    sqlx::query("UPDATE _sqlx_migrations SET checksum = zeroblob(48) WHERE version = 1")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);

    let err = init_db(&url)
        .await
        .expect_err("a tampered migration checksum must fail startup");
    assert!(
        matches!(err, InitDbError::Migrate(_)),
        "expected InitDbError::Migrate, got {err:?}"
    );
}

#[tokio::test]
async fn migrator_records_applied_versions() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let versions: Vec<i64> =
        sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
            .fetch_all(&pool)
            .await
            .expect("_sqlx_migrations should exist after init_db");
    assert!(
        versions.contains(&1),
        "baseline migration 0001 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&2),
        "normalized migration 0002 should be recorded, got {versions:?}"
    );
    assert!(
        versions.contains(&3),
        "legacy-drop migration 0003 should be recorded, got {versions:?}"
    );
}

#[tokio::test]
async fn migration_0021_drops_redundant_and_dead_schema_objects() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // F17 + F18: the four redundant indexes and the speculative
    // NULL-accent partial index must be gone after the migrator runs.
    for index in [
        "idx_books_uuid",
        "idx_book_files_book_id",
        "idx_book_file_parts_lookup",
        "reading_progress_user_book_idx",
        "idx_books_accent_null",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?")
                .bind(index)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(count, 0, "index {index} should have been dropped by 0021");
    }

    // F20: the vestigial single-row table must be gone.
    let app_state: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='app_state'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        app_state, 0,
        "app_state table should have been dropped by 0021"
    );

    // The covering composite index that supersedes idx_book_files_book_id
    // must still be present (kept, not dropped).
    let kept_composite: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='idx_book_files_book_format'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        kept_composite, 1,
        "idx_book_files_book_format must survive — it covers book_id-only lookups"
    );

    // The (user_id, book_id) session indexes from 0013 have no covering
    // UNIQUE, so they must NOT have been dropped.
    for kept in [
        "bookmarks_user_book_idx",
        "reading_sessions_user_book_idx",
        "listening_sessions_user_book_idx",
    ] {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name=?")
                .bind(kept)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "index {kept} has no covering UNIQUE and must be kept"
        );
    }

    // The UNIQUE(user_id, book_uuid, format) auto-index (the soft-ref key
    // post-F1) must still enforce uniqueness: a duplicate
    // (user_id, book_uuid, format) insert must fail.
    sqlx::query("INSERT INTO users (username, password_hash, is_admin) VALUES ('u', 'h', 0)")
        .execute(&pool)
        .await
        .unwrap();
    // `libraries` was renamed to `scan_roots` in 0019; `books.library_id`
    // keeps its column name and FK.
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('bk-uuid', 1, '/lib/bk', 'Book')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi) \
         VALUES (1, 'bk-uuid', 'epub', 'cfi-1')",
    )
    .execute(&pool)
    .await
    .unwrap();
    let dup = sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi) \
         VALUES (1, 'bk-uuid', 'epub', 'cfi-2')",
    )
    .execute(&pool)
    .await;
    assert!(
        dup.is_err(),
        "UNIQUE(user_id, book_uuid, format) must still reject a duplicate row"
    );
}

#[tokio::test]
async fn migration_0038_stores_machine_timestamps_as_integer_unix_seconds() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Minimum FK graph, then one row per table 0038 migrated. All rely on the
    // (now epoch) column defaults except `books`, whose in-place conversion
    // dropped its default — so its two columns are set explicitly.
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'h', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, timestamp, last_modified) \
         VALUES ('bk', 1, '/lib/bk', 'Book', strftime('%s','now'), strftime('%s','now'))",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authors (id, name) VALUES (1, 'Ada')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, updated_by) VALUES ('bk', '{}', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, bytes, mime) \
         VALUES (1, 'openlibrary', 'http://x', X'00', 'image/png')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO merge_log (target_book_id, source_uuid, source_metadata) \
         VALUES ((SELECT id FROM books WHERE uuid='bk'), 'src', '{}')",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO ignored_authors (name) VALUES ('Bob')")
        .execute(&pool)
        .await
        .unwrap();

    // Every migrated machine-timestamp column is INTEGER-typed and a plausible
    // "now" (> 2023-11-14). `typeof` proves the storage class, not just affinity.
    for (sql, label) in [
        (
            "SELECT typeof(timestamp) || ':' || (timestamp > 1700000000) FROM books",
            "books.timestamp",
        ),
        (
            "SELECT typeof(last_modified) || ':' || (last_modified > 1700000000) FROM books",
            "books.last_modified",
        ),
        (
            "SELECT typeof(updated_at) || ':' || (updated_at > 1700000000) FROM metadata_overrides",
            "metadata_overrides.updated_at",
        ),
        (
            "SELECT typeof(fetched_at) || ':' || (fetched_at > 1700000000) FROM author_photos",
            "author_photos.fetched_at",
        ),
        (
            "SELECT typeof(merged_at) || ':' || (merged_at > 1700000000) FROM merge_log",
            "merge_log.merged_at",
        ),
        (
            "SELECT typeof(ignored_at) || ':' || (ignored_at > 1700000000) FROM ignored_authors",
            "ignored_authors.ignored_at",
        ),
    ] {
        let got: String = sqlx::query_scalar(sql).fetch_one(&pool).await.unwrap();
        assert_eq!(
            got, "integer:1",
            "{label} must be INTEGER unix-seconds, got {got}"
        );
    }
}

#[tokio::test]
async fn migration_0038_text_datetime_converts_to_exact_unix_seconds() {
    // The in-DDL backfill relies on this exact conversion of the old
    // `datetime('now')` / `CURRENT_TIMESTAMP` TEXT form. Locking it here is the
    // parsing case rule 03 requires; it would fail on the pre-0038 schema where
    // these columns stayed TEXT and sorted lexicographically.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let epoch: i64 =
        sqlx::query_scalar("SELECT CAST(strftime('%s','2024-01-02 03:04:05') AS INTEGER)")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(epoch, 1_704_164_645);
}

#[tokio::test]
async fn migration_0038_recreate_drops_orphan_author_photos() {
    // Real databases carry rows that violate a declared FK because the parent
    // was removed while enforcement was off — e.g. GC'd authors leaving orphan
    // `author_photos`. 0038's table recreates re-validate every FK under
    // enforcement, so the copy must repair such orphans (drop the dead
    // CASCADE rows) instead of aborting with `FOREIGN KEY constraint failed`.
    // `init_db` runs 0038 atomically on an empty DB and can't hold a
    // pre-migration orphan, so this reconstructs the minimal shape and drives
    // the same orphan-filtering recreate pattern 0038 uses.
    //
    // A pool over `sqlite::memory:` hands each connection its own DB, so every
    // statement must run on one acquired connection.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE authors (id INTEGER PRIMARY KEY, name TEXT)",
        "CREATE TABLE author_photos (author_id INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE, \
         source TEXT NOT NULL, url TEXT, bytes BLOB, mime TEXT, fetched_at TEXT NOT NULL DEFAULT (datetime('now')))",
        "INSERT INTO authors (id, name) VALUES (1, 'Ada')",
        "INSERT INTO author_photos (author_id, source) VALUES (1, 'letter')",
        // Orphan: author 99 doesn't exist. Insert under FK-off, mimicking a
        // parent deletion that ran without cascade enforcement.
        "PRAGMA foreign_keys=OFF",
        "INSERT INTO author_photos (author_id, source) VALUES (99, 'letter')",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE author_photos_new (author_id INTEGER PRIMARY KEY REFERENCES authors(id) ON DELETE CASCADE, \
         source TEXT NOT NULL, url TEXT, bytes BLOB, mime TEXT, fetched_at INTEGER NOT NULL DEFAULT (strftime('%s','now')))",
        "INSERT INTO author_photos_new \
           SELECT author_id, source, url, bytes, mime, CAST(strftime('%s', fetched_at) AS INTEGER) \
             FROM author_photos ap WHERE EXISTS (SELECT 1 FROM authors a WHERE a.id = ap.author_id)",
        "DROP TABLE author_photos",
        "ALTER TABLE author_photos_new RENAME TO author_photos",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-filtering recreate must not hit a FK violation");
    }

    let ids: Vec<i64> =
        sqlx::query_scalar("SELECT author_id FROM author_photos ORDER BY author_id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(ids, vec![1], "orphan row dropped, valid row kept");
}

#[tokio::test]
async fn migration_0038_recreate_coerces_orphan_metadata_updated_by_to_null() {
    // `metadata_overrides.updated_by` is `ON DELETE SET NULL`: a dangling
    // reference (user deleted without enforcement) must become NULL on the
    // recreate's `LEFT JOIN users`, keeping the row rather than aborting.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT)",
        "CREATE TABLE metadata_overrides (book_uuid TEXT NOT NULL PRIMARY KEY, overrides TEXT NOT NULL DEFAULT '{}', \
         has_cover_override INTEGER NOT NULL DEFAULT 0, updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         updated_at TEXT NOT NULL DEFAULT (datetime('now')))",
        "INSERT INTO users (id, username) VALUES (1, 'admin')",
        "INSERT INTO metadata_overrides (book_uuid, updated_by) VALUES ('valid', 1)",
        "PRAGMA foreign_keys=OFF",
        "INSERT INTO metadata_overrides (book_uuid, updated_by) VALUES ('orphan', 99)",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE metadata_overrides_new (book_uuid TEXT NOT NULL PRIMARY KEY, overrides TEXT NOT NULL DEFAULT '{}', \
         has_cover_override INTEGER NOT NULL DEFAULT 0, updated_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         updated_at INTEGER NOT NULL DEFAULT (strftime('%s','now')))",
        "INSERT INTO metadata_overrides_new \
           SELECT mo.book_uuid, mo.overrides, mo.has_cover_override, u.id, CAST(strftime('%s', mo.updated_at) AS INTEGER) \
             FROM metadata_overrides mo LEFT JOIN users u ON u.id = mo.updated_by",
        "DROP TABLE metadata_overrides",
        "ALTER TABLE metadata_overrides_new RENAME TO metadata_overrides",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-coercing recreate must not hit a FK violation");
    }

    let rows: Vec<(String, Option<i64>)> =
        sqlx::query_as("SELECT book_uuid, updated_by FROM metadata_overrides ORDER BY book_uuid")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![("orphan".into(), None), ("valid".into(), Some(1))],
        "both rows kept; the dangling updated_by is coerced to NULL"
    );
}

#[tokio::test]
async fn migration_0038_recreate_drops_orphan_merge_log_and_coerces_merged_by() {
    // `merge_log.target_book_id` is NOT NULL CASCADE — an orphan (book gone)
    // must be dropped by the inner `JOIN books`. `merged_by` is SET NULL — a
    // dangling ref must survive as NULL via the `LEFT JOIN users`.
    let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
    let mut conn = pool.acquire().await.unwrap();
    for setup in [
        "PRAGMA foreign_keys=ON",
        "CREATE TABLE users (id INTEGER PRIMARY KEY, username TEXT)",
        "CREATE TABLE books (id INTEGER PRIMARY KEY, title TEXT)",
        "CREATE TABLE merge_log (id INTEGER PRIMARY KEY AUTOINCREMENT, \
         target_book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, source_uuid TEXT NOT NULL, \
         source_metadata TEXT NOT NULL, merged_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         merged_at TEXT NOT NULL DEFAULT (datetime('now')), undone_at TEXT)",
        "INSERT INTO users (id, username) VALUES (1, 'admin')",
        "INSERT INTO books (id, title) VALUES (1, 'Book')",
        // Valid row (target 1, merged_by 1).
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (1, 1, 'a', '{}', 1)",
        "PRAGMA foreign_keys=OFF",
        // Orphan target (book 99 gone) -> dropped; dangling merged_by (user 99) -> coerced.
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (2, 99, 'b', '{}', 1)",
        "INSERT INTO merge_log (id, target_book_id, source_uuid, source_metadata, merged_by) VALUES (3, 1, 'c', '{}', 99)",
        "PRAGMA foreign_keys=ON",
    ] {
        sqlx::query(setup).execute(&mut *conn).await.unwrap();
    }

    for stmt in [
        "CREATE TABLE merge_log_new (id INTEGER PRIMARY KEY AUTOINCREMENT, \
         target_book_id INTEGER NOT NULL REFERENCES books(id) ON DELETE CASCADE, source_uuid TEXT NOT NULL, \
         source_metadata TEXT NOT NULL, merged_by INTEGER REFERENCES users(id) ON DELETE SET NULL, \
         merged_at INTEGER NOT NULL DEFAULT (strftime('%s','now')), undone_at INTEGER)",
        "INSERT INTO merge_log_new \
           SELECT ml.id, ml.target_book_id, ml.source_uuid, ml.source_metadata, u.id, \
                  CAST(strftime('%s', ml.merged_at) AS INTEGER), \
                  CASE WHEN ml.undone_at IS NULL THEN NULL ELSE CAST(strftime('%s', ml.undone_at) AS INTEGER) END \
             FROM merge_log ml JOIN books b ON b.id = ml.target_book_id LEFT JOIN users u ON u.id = ml.merged_by",
        "DROP TABLE merge_log",
        "ALTER TABLE merge_log_new RENAME TO merge_log",
    ] {
        sqlx::query(stmt)
            .execute(&mut *conn)
            .await
            .expect("orphan-repairing recreate must not hit a FK violation");
    }

    let rows: Vec<(i64, Option<i64>)> =
        sqlx::query_as("SELECT id, merged_by FROM merge_log ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .unwrap();
    assert_eq!(
        rows,
        vec![(1, Some(1)), (3, None)],
        "orphan-target row 2 dropped; row 3's dangling merged_by coerced to NULL"
    );
}

#[tokio::test]
async fn migrator_is_idempotent_on_rerun() {
    let tmp = std::env::temp_dir().join(format!(
        "omnibus-migrate-{}-{}.db",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_file(&tmp);
    let url = format!("sqlite://{}?mode=rwc", tmp.display());

    let pool1 = init_db(&url).await.expect("first init");
    drop(pool1);
    let pool2 = init_db(&url).await.expect("second init");

    let by_version: Vec<(i64, i64)> =
        sqlx::query_as("SELECT version, COUNT(*) FROM _sqlx_migrations GROUP BY version")
            .fetch_all(&pool2)
            .await
            .unwrap();
    for (_, count) in by_version {
        assert_eq!(count, 1, "every migration recorded exactly once");
    }

    drop(pool2);
    let _ = std::fs::remove_file(&tmp);
}

/// Migration 0070 plus the boot backfill re-derive the keys the `&`
/// expansion invalidated. Simulates an upgrade over an existing database:
/// rows written by the old folder (which dropped `&`) and a
/// `_sqlx_migrations` table that has not yet seen 0070, then a second
/// `init_db` over the same file.
///
/// Covers all four shapes the two arms have to get right: a `&` title with a
/// link, a `&` title *without* one (the blocklisted-first-creator gap, where
/// the stored author key is the only copy and must survive), a `&` in the
/// position-0 author's name, and a row with no `&` at all.
#[tokio::test]
async fn migration_0070_recomputes_stale_ampersand_norms_on_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = init_db(&url).await.expect("initial init_db should succeed");

    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    // Every `*_norm` seeded here is what the pre-change folder wrote: the
    // ampersand collapsed to a space like any other punctuation. The two
    // `sentinel` keys are deliberately wrong, so any recompute shows up.
    let ampersand_title = "A Tale of Mirth & Magic";
    seed_pre_ampersand_book(
        &pool,
        "u1",
        ampersand_title,
        "a tale of mirth magic",
        "ada quill",
        Some("Ada Quill"),
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u2",
        "Dracula",
        "sentinel",
        "sentinel",
        Some("Ada Quill"),
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u3",
        ampersand_title,
        "a tale of mirth magic",
        "blocklisted quill",
        None,
    )
    .await;
    seed_pre_ampersand_book(
        &pool,
        "u4",
        "Duet",
        "duet",
        "vale quill",
        Some("Vale & Quill"),
    )
    .await;
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, title_norm, author_norm)
         VALUES ('u1', '{\"title\":\"A Tale of Mirth & Magic\"}', 'a tale of mirth magic', NULL)",
    )
    .execute(&pool)
    .await
    .unwrap();

    // Hand the database back its pre-0070 migration state and re-open it.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 70")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let pool = init_db(&url).await.expect("upgrade init_db should succeed");

    assert_eq!(
        norms(&pool, "u1").await,
        ("a tale of mirth and magic".into(), Some("ada quill".into())),
        "a `&` title heals; its derivable author key is unchanged"
    );
    assert_eq!(
        norms(&pool, "u2").await,
        ("sentinel".into(), Some("sentinel".into())),
        "a row with no `&` on either side is not reset at all"
    );
    assert_eq!(
        norms(&pool, "u3").await,
        (
            "a tale of mirth and magic".into(),
            Some("blocklisted quill".into())
        ),
        "a `&` title must not cost a book its non-derivable author key"
    );
    assert_eq!(
        norms(&pool, "u4").await,
        ("duet".into(), Some("vale and quill".into())),
        "a `&` in the position-0 author's name heals the author key"
    );
    // The override keys need no migration — their boot pass recomputes every
    // row from the `overrides` JSON and rewrites the ones that disagree.
    let override_norm: String =
        sqlx::query_scalar("SELECT title_norm FROM metadata_overrides WHERE book_uuid = 'u1'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(override_norm, "a tale of mirth and magic");
}

/// #1912 / AC3: pre-fix rows whose series metadata carried the index inside
/// the name ("Crowns of Nyaxia #1"/"#2"/"#3") fragmented into three
/// one-book series, `series_index` never filled. Migration 0071 nulls the
/// stale `series_sort` those rows already hold and the boot backfills
/// (`series_normalize::backfill_embedded_series_index`, then
/// `sort_keys::backfill_series_sort`) merge the fragmented `series` rows
/// into one and fill each book's index. Simulates an upgrade the same way
/// `migration_0070_recomputes_stale_ampersand_norms_on_upgrade` does: rows
/// seeded as the pre-fix folder wrote them, then a second `init_db` over the
/// same file with 0071 rewound.
#[tokio::test]
async fn migration_0071_merges_stale_embedded_index_series_on_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("omnibus.db");
    let url = format!("sqlite://{}?mode=rwc", db_path.display());
    let pool = init_db(&url).await.expect("initial init_db should succeed");

    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    for (uuid, n) in [("u1", 1), ("u2", 2), ("u3", 3)] {
        let series_name = format!("Crowns of Nyaxia #{n}");
        let series_id: i64 =
            sqlx::query_scalar("INSERT INTO series (name) VALUES (?1) RETURNING id")
                .bind(&series_name)
                .fetch_one(&pool)
                .await
                .unwrap();
        let book_id: i64 = sqlx::query_scalar(
            "INSERT INTO books (uuid, library_id, path, title, series_sort)
             VALUES (?1, ?2, '', ?3, ?4) RETURNING id",
        )
        .bind(uuid)
        .bind(lib_id)
        .bind(format!("Book {n}"))
        .bind(&series_name)
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?1, ?2)")
            .bind(book_id)
            .bind(series_id)
            .execute(&pool)
            .await
            .unwrap();
    }

    // Hand the database back its pre-0071 migration state and re-open it.
    sqlx::query("DELETE FROM _sqlx_migrations WHERE version = 71")
        .execute(&pool)
        .await
        .unwrap();
    drop(pool);
    let pool = init_db(&url).await.expect("upgrade init_db should succeed");

    let series_rows: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series_rows.len(),
        1,
        "AC3: the three fragmented rows converge on one series: {series_rows:?}"
    );
    assert_eq!(series_rows[0].1, "Crowns of Nyaxia");

    let mut rows: Vec<(String, f64, Option<String>)> =
        sqlx::query_as("SELECT title, series_index, series_sort FROM books ORDER BY series_index")
            .fetch_all(&pool)
            .await
            .unwrap();
    rows.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap());
    assert_eq!(
        rows,
        vec![
            (
                "Book 1".to_string(),
                1.0,
                Some("Crowns of Nyaxia".to_string())
            ),
            (
                "Book 2".to_string(),
                2.0,
                Some("Crowns of Nyaxia".to_string())
            ),
            (
                "Book 3".to_string(),
                3.0,
                Some("Crowns of Nyaxia".to_string())
            ),
        ],
        "each book keeps its parsed index and gets the cleaned series_sort"
    );
}

/// Seed a book carrying hand-written `_norm` keys, standing in for a row the
/// pre-`&`-expansion folder wrote. `link_author` is the position-0 author link
/// the boot backfill re-derives `author_norm` from; `None` reproduces the
/// blocklisted-first-creator gap, where the link is absent and the stored key
/// is the only copy of it.
async fn seed_pre_ampersand_book(
    pool: &SqlitePool,
    uuid: &str,
    title: &str,
    title_norm: &str,
    author_norm: &str,
    link_author: Option<&str>,
) {
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, title_norm, author_norm)
         VALUES (?1, (SELECT id FROM scan_roots WHERE path = '/lib'), '', ?2, ?3, ?4)
         RETURNING id",
    )
    .bind(uuid)
    .bind(title)
    .bind(title_norm)
    .bind(author_norm)
    .fetch_one(pool)
    .await
    .unwrap();
    let Some(name) = link_author else { return };
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?1)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO books_authors_link (book, author, position)
         VALUES (?1, (SELECT id FROM authors WHERE name = ?2), 0)",
    )
    .bind(book_id)
    .bind(name)
    .execute(pool)
    .await
    .unwrap();
}

/// The stored `(title_norm, author_norm)` pair for one book.
async fn norms(pool: &SqlitePool, uuid: &str) -> (String, Option<String>) {
    sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE uuid = ?1")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

struct GhostedRepairFixture {
    audio_path: String,
    target_id: i64,
    target_uuid: String,
    healthy_uuid: String,
}

async fn seed_ghosted_repair_fixture(pool: &SqlitePool) -> GhostedRepairFixture {
    let audio_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("audiobooks")
        .join("generated")
        .join("grace_hopper_series");
    let audio_path = audio_root.to_string_lossy().into_owned();
    let target_uuid = crate::test_support::seed_synced_ebook(
        pool,
        "Hopper/The Compiled Tales.epub",
        "The Compiled Tales",
        "Grace Hopper",
    )
    .await;
    let healthy_uuid = crate::test_support::seed_synced_ebook(
        pool,
        "Lovelace/Notes.epub",
        "Notes",
        "Ada Lovelace",
    )
    .await;
    crate::sync::sync_audiobooks(
        pool,
        "/healthy-audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                "Lovelace/Notes.m4b",
                "Notes",
                Some("Ada Lovelace"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    crate::indexer::reindex_audiobooks(pool, &audio_path)
        .await
        .unwrap();
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    let audio_file_id: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND format = 'MP3'")
            .bind(target_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let initial_parts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = ?")
            .bind(audio_file_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(initial_parts, 2, "fixture must begin as a two-part book");

    sqlx::query("DELETE FROM book_file_parts WHERE book_file_id = ? AND ordinal = 0")
        .bind(audio_file_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ghosted-chapter-1', ?, 'MP3', ?, 'the_compiled_tales/chapter01.mp3')",
    )
    .bind(target_id)
    .bind(&audio_path)
    .execute(pool)
    .await
    .unwrap();

    seed_repair_user_data(pool, &target_uuid).await;
    assert_eq!(ghosted_slot_counts(pool, target_id).await, (2, 1));

    GhostedRepairFixture {
        audio_path,
        target_id,
        target_uuid,
        healthy_uuid,
    }
}

async fn seed_repair_user_data(pool: &SqlitePool, target_uuid: &str) {
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin)
         VALUES (1, 'repair-user', 'hash', 1)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, audio_position_seconds)
         VALUES (1, ?, 'audio', 321.5)",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars)
         VALUES (1, ?, 9)",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md)
         VALUES (1, ?, 'Preserve this note')",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
}

async fn ghosted_slot_counts(pool: &SqlitePool, target_id: i64) -> (i64, i64) {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM merged_uuids
               WHERE book_id = ? AND format = 'MP3'),
             (SELECT COUNT(*) FROM book_files
               WHERE book_id = ? AND format = 'MP3')",
    )
    .bind(target_id)
    .bind(target_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn repair_user_data_counts(pool: &SqlitePool, target_uuid: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM reading_progress WHERE book_uuid = ?),
             (SELECT COUNT(*) FROM user_ratings WHERE book_uuid = ?),
             (SELECT COUNT(*) FROM journal_entries WHERE book_uuid = ?)",
    )
    .bind(target_uuid)
    .bind(target_uuid)
    .bind(target_uuid)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn boot_backfill_repairs_ghosted_multi_attach_and_preserves_surviving_book_data() {
    let _covers = crate::test_support::CoversTempDir::new("ghosted-multi-attach-repair");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let fixture = seed_ghosted_repair_fixture(&pool).await;

    run_boot_backfills(&pool).await.unwrap();
    let repaired_slot = ghosted_slot_counts(&pool, fixture.target_id).await;
    assert_eq!(
        repaired_slot,
        (0, 0),
        "repair must clear the corrupt attachment slot so disk files re-flow"
    );
    run_boot_backfills(&pool).await.unwrap();
    assert_eq!(
        ghosted_slot_counts(&pool, fixture.target_id).await,
        repaired_slot
    );
    let preserved_books: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM books WHERE uuid = ?),
             (SELECT COUNT(*) FROM book_files bf
               JOIN books b ON b.id = bf.book_id
              WHERE b.uuid = ?),
             (SELECT COUNT(*) FROM merged_uuids
               WHERE book_id = (SELECT id FROM books WHERE uuid = ?))",
    )
    .bind(&fixture.target_uuid)
    .bind(&fixture.healthy_uuid)
    .bind(&fixture.healthy_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved_books, (1, 2, 1));
    assert_eq!(
        repair_user_data_counts(&pool, &fixture.target_uuid).await,
        (1, 1, 1)
    );

    crate::indexer::reindex_audiobooks(&pool, &fixture.audio_path)
        .await
        .unwrap();
    let rebuilt: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM books WHERE uuid = ?),
             (SELECT COUNT(*) FROM book_files
               WHERE book_id = ? AND format = 'MP3'),
             (SELECT COUNT(*) FROM book_file_parts p
               JOIN book_files bf ON bf.id = p.book_file_id
              WHERE bf.book_id = ? AND bf.format = 'MP3')",
    )
    .bind(&fixture.target_uuid)
    .bind(fixture.target_id)
    .bind(fixture.target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rebuilt,
        (1, 1, 2),
        "the surviving book identity must regain one two-part audio file"
    );
    assert_eq!(
        repair_user_data_counts(&pool, &fixture.target_uuid).await,
        (1, 1, 1)
    );
}

/// Forge a per-file guard and replay the file so it attaches as another
/// same-format part under `book_id` (a manual multi-part merge).
async fn attach_m4b_part(pool: &SqlitePool, book_id: i64, group_path: &str) {
    let ab =
        crate::test_support::indexed_audiobook(group_path, "Wind and Truth", Some("Sanderson"));
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, 'M4B', '/audio', ?)",
    )
    .bind(crate::helpers::stable_uuid("/audio", group_path))
    .bind(book_id)
    .bind(&ab.scan_key)
    .execute(pool)
    .await
    .unwrap();
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
}

/// Insert a resurrected standalone book for `group_path` (the bug's output):
/// a books row whose scan_key equals an already-attached part's file.
async fn resurrect_standalone(pool: &SqlitePool, group_path: &str) {
    crate::sync::sync_audiobooks(
        pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                group_path,
                "Wind and Truth",
                Some("Sanderson"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn boot_backfill_replants_multipart_guards_and_drops_resurrected_duplicates() {
    let _covers = crate::test_support::CoversTempDir::new("multipart-guard-repair");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid = crate::test_support::seed_synced_ebook(
        &pool,
        "Sanderson/wt.epub",
        "Wind and Truth",
        "Sanderson",
    )
    .await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    for gp in [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
    ] {
        attach_m4b_part(&pool, target_id, gp).await;
    }

    // Corrupt to the pre-#1126 state: parts 2 & 3 lost their guards and
    // resurrected as standalone books; part 3's dupe accrued user data.
    sqlx::query(
        "DELETE FROM merged_uuids WHERE scan_key IN ('Sanderson/wt-2.m4b', 'Sanderson/wt-3.m4b')",
    )
    .execute(&pool)
    .await
    .unwrap();
    resurrect_standalone(&pool, "Sanderson/wt-2.m4b").await;
    resurrect_standalone(&pool, "Sanderson/wt-3.m4b").await;
    let dupe3_uuid: String =
        sqlx::query_scalar("SELECT uuid FROM books WHERE scan_key = 'Sanderson/wt-3.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'h', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (1, ?, 8)")
        .bind(&dupe3_uuid)
        .execute(&pool)
        .await
        .unwrap();
    // Precondition: 1 target + 2 standalone dupes; only part 1 keeps its guard.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 3);
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        1
    );

    run_boot_backfills(&pool).await.unwrap();

    // Every attached part regains a guard; the no-user-data dupe (part 2) is
    // deleted, but the rated dupe (part 3) is left for a deliberate merge.
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books WHERE scan_key = 'Sanderson/wt-2.m4b'"
        )
        .await,
        0,
        "the unrated resurrected duplicate is removed"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books WHERE scan_key = 'Sanderson/wt-3.m4b'"
        )
        .await,
        1,
        "a dupe carrying user data is preserved for manual handling"
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM user_ratings").await, 1);

    // Idempotent: a second run changes nothing.
    run_boot_backfills(&pool).await.unwrap();
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        3
    );

    // The three parts now classify Unchanged on a reindex — no resurrection.
    let disk: Vec<_> = [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
    ]
    .iter()
    .map(|gp| {
        let ab = crate::test_support::indexed_audiobook(gp, "Wind and Truth", Some("Sanderson"));
        crate::ebook::StatEntry {
            filename: ab.group_path.clone(),
            scan_key: ab.scan_key.clone(),
            mtime_epoch: ab.max_mtime_epoch,
            size_bytes: ab.total_size_bytes,
            error: None,
        }
    })
    .collect();
    let db_rows =
        crate::books::list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
            .await
            .unwrap();
    let diff = crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    assert_eq!(diff.unchanged.len(), 3);
    assert!(diff.new.is_empty());
}

#[tokio::test]
async fn boot_repair_spares_a_same_scan_key_book_in_another_library() {
    // scan_key is only unique per (library_id, scan_key). A healthy book in
    // library B must not be deleted just because library A attached a file at
    // the same *relative* path — the dupe match is scoped to the same root.
    let _covers = crate::test_support::CoversTempDir::new("multipart-guard-crosslib");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Library A ("/audio-a"): an ebook with an attached m4b at "Dup/p.m4b".
    let target_uuid =
        crate::test_support::seed_synced_ebook(&pool, "A/book.epub", "Book A", "Author A").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let ab = crate::test_support::indexed_audiobook("Dup/p.m4b", "Book A", Some("Author A"));
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, 'M4B', '/audio-a', ?)",
    )
    .bind(crate::helpers::stable_uuid("/audio-a", "Dup/p.m4b"))
    .bind(target_id)
    .bind(&ab.scan_key)
    .execute(&pool)
    .await
    .unwrap();
    crate::sync::sync_audiobooks(
        &pool,
        "/audio-a",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Library B ("/audio-b"): a healthy standalone audiobook at the same
    // relative path, different title/author, no user data.
    crate::sync::sync_audiobooks(
        &pool,
        "/audio-b",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                "Dup/p.m4b",
                "Book B",
                Some("Author B"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    run_boot_backfills(&pool).await.unwrap();

    // The library-B book survives — its file lives under a different scan root.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books b JOIN scan_roots l ON l.id = b.library_id
              WHERE b.scan_key = 'Dup/p.m4b' AND l.path = '/audio-b'"
        )
        .await,
        1
    );
}

#[test]
fn purge_legacy_covers_once_sweeps_then_no_ops() {
    // `CoversTempDir` pins `OMNIBUS_COVERS_DIR` at a unique temp path under
    // the shared env lock and removes it on drop; the purge takes the dir as
    // a parameter, so this asserts the function in isolation from `init_db`.
    let covers = CoversTempDir::new("purge_sweep");
    std::fs::create_dir_all(&covers.path).unwrap();

    // Seed three "legacy" cover files.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        std::fs::write(covers.path.join(name), b"x").unwrap();
    }

    purge_legacy_covers_once(&covers.path);

    // Legacy files gone, sentinel written.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        assert!(
            !covers.path.join(name).exists(),
            "legacy file {name} should have been purged",
        );
    }
    assert!(
        covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "sentinel should be present after first purge",
    );

    // A freshly-written cover after the purge must survive a second
    // call — the sentinel short-circuits the sweep.
    let kept = covers.path.join("dddd.jpg");
    std::fs::write(&kept, b"y").unwrap();
    purge_legacy_covers_once(&covers.path);
    assert!(
        kept.exists(),
        "post-sentinel cover writes must not be deleted",
    );
}

#[test]
fn purge_legacy_covers_once_creates_and_marks_a_missing_dir() {
    // Cold boot before any covers have ever been written: nothing to purge,
    // but the scheme still has to be marked, so the dir is created for the
    // sentinel to live in.
    let covers = CoversTempDir::new("purge_missing");
    assert!(
        !covers.path.exists(),
        "precondition: dir does not exist yet"
    );

    purge_legacy_covers_once(&covers.path);

    assert!(
        covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "a missing covers dir must still be created and marked",
    );
    let entries: Vec<_> = std::fs::read_dir(&covers.path).unwrap().flatten().collect();
    assert_eq!(
        entries.len(),
        1,
        "the sentinel should be the only thing written",
    );
}

#[test]
fn purge_legacy_covers_once_leaves_covers_extracted_after_a_missing_dir_boot() {
    // The fresh-install regression: boot 1 finds no covers dir, boot 1's
    // indexing run then extracts every cover into it, and boot 2 must not
    // mistake that populated cache for an unswept legacy one.
    let covers = CoversTempDir::new("purge_fresh_install");

    // Boot 1: no dir yet.
    purge_legacy_covers_once(&covers.path);

    // Boot 1's indexer extracts covers into the now-existing dir.
    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        std::fs::write(covers.path.join(name), b"x").unwrap();
    }

    // Boot 2.
    purge_legacy_covers_once(&covers.path);

    for name in ["aaaa.jpg", "bbbb.png", "cccc.webp"] {
        assert!(
            covers.path.join(name).exists(),
            "cover {name} extracted after the first boot must survive the second",
        );
    }
}

#[cfg(unix)]
#[test]
fn purge_legacy_covers_once_leaves_scheme_unmarked_when_a_file_cannot_be_removed() {
    use std::os::unix::fs::PermissionsExt;

    // A partial sweep must not write the sentinel: marking a dir that still
    // holds a legacy file would orphan that file forever.
    let covers = CoversTempDir::new("purge_readonly");
    std::fs::create_dir_all(&covers.path).unwrap();
    let stuck = covers.path.join("aaaa.jpg");
    std::fs::write(&stuck, b"x").unwrap();

    // A read-only directory rejects the unlink of an entry inside it.
    std::fs::set_permissions(&covers.path, std::fs::Permissions::from_mode(0o555)).unwrap();
    purge_legacy_covers_once(&covers.path);
    // Restore before asserting so the temp dir is still removable on drop.
    std::fs::set_permissions(&covers.path, std::fs::Permissions::from_mode(0o755)).unwrap();

    // Running as root ignores the mode bits, so the unlink succeeds and there
    // is no partial sweep to assert on.
    if stuck.exists() {
        assert!(
            !covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
            "a sweep that left a legacy file behind must leave the scheme unmarked",
        );
    }
}

#[cfg(unix)]
#[test]
fn purge_legacy_covers_once_leaves_scheme_unmarked_when_an_entry_cannot_be_statted() {
    // An entry the sweep can't stat is one it can't verify or remove, so the
    // sentinel must not land. A dangling symlink is the reproducible case:
    // `std::fs::metadata` follows the link and errors on the missing target.
    let covers = CoversTempDir::new("purge_unstattable");
    std::fs::create_dir_all(&covers.path).unwrap();
    std::os::unix::fs::symlink(
        covers.path.join("no-such-target"),
        covers.path.join("dangling.jpg"),
    )
    .unwrap();

    purge_legacy_covers_once(&covers.path);

    assert!(
        !covers.path.join(COVERS_SCHEME_SENTINEL).exists(),
        "a sweep with an unstattable entry must leave the scheme unmarked",
    );
}

#[tokio::test]
async fn migration_0069_creates_library_cleanup_tables_with_unique_constraints() {
    // AC1: a fresh in-memory DB applies migration 0069 cleanly and all three
    // Library Cleanup tables accept a well-formed row.
    let pool = init_db("sqlite::memory:").await.unwrap();

    sqlx::query(
        "INSERT INTO dedup_suggestions (kind, action, payload_json) VALUES ('author', 'merge', '{\"a\":1}')",
    )
    .execute(&pool)
    .await
    .expect("dedup_suggestions should accept a well-formed row");

    sqlx::query(
        "INSERT INTO cleanup_log (kind, action, snapshot_json) VALUES ('author', 'merge', '{\"before\":1}')",
    )
    .execute(&pool)
    .await
    .expect("cleanup_log should accept a well-formed row");

    sqlx::query("INSERT INTO entity_aliases (kind, alias_name, canonical_id) VALUES ('author', 'Ursula Le Guin', 1)")
        .execute(&pool)
        .await
        .expect("entity_aliases should accept a well-formed row");

    for (table, expected) in [
        ("dedup_suggestions", 1),
        ("cleanup_log", 1),
        ("entity_aliases", 1),
    ] {
        let n = count(&pool, &format!("SELECT COUNT(*) FROM {table}")).await;
        assert_eq!(n, expected, "{table} should hold exactly one row");
    }
}

#[tokio::test]
async fn migration_0069_dedup_suggestions_rejects_duplicate_kind_action_payload() {
    // AC2: re-inserting an identical (kind, action, payload_json) row is a
    // constraint violation, so a re-run detector job can safely no-op via
    // `INSERT OR IGNORE` rather than duplicating a pending suggestion.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let insert = "INSERT INTO dedup_suggestions (kind, action, payload_json) VALUES ('tag', 'merge', '{\"from\":\"scifi\",\"to\":\"sci-fi\"}')";

    sqlx::query(insert)
        .execute(&pool)
        .await
        .expect("first insert should succeed");

    let err = sqlx::query(insert)
        .execute(&pool)
        .await
        .expect_err("duplicate (kind, action, payload_json) must violate the UNIQUE constraint");
    assert!(
        err.to_string().to_lowercase().contains("constraint"),
        "got: {err}"
    );
}

#[tokio::test]
async fn migration_0069_entity_aliases_rejects_duplicate_kind_alias_name() {
    // (kind, alias_name) is the primary key: the same alias can't map to two
    // different canonical ids within the same kind.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let insert =
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id) VALUES ('series', 'Foundation', ?)";

    sqlx::query(insert)
        .bind(1)
        .execute(&pool)
        .await
        .expect("first insert should succeed");

    let err = sqlx::query(insert)
        .bind(2)
        .execute(&pool)
        .await
        .expect_err("duplicate (kind, alias_name) must violate the primary key");
    assert!(
        err.to_string().to_lowercase().contains("constraint"),
        "got: {err}"
    );
}
